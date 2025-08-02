use alloy::primitives::TxHash;
use alloy::providers::ext::AnvilApi;
use alloy::{primitives::U256, providers::Provider};

use bitcoincore_rpc_async::RpcApi;
use devnet::bitcoin_devnet::MiningMode;
use devnet::{MultichainAccount, RiftDevnet};
use evm_token_indexer_client::TokenIndexerClient;
use market_maker::evm_wallet::EVMWallet;
use market_maker::wallet::Wallet;
use market_maker::{bitcoin_wallet::BitcoinWallet, run_market_maker, MarketMakerArgs};
use otc_models::{ChainType, Currency, Quote, TokenIdentifier};
use otc_server::api::SwapResponse;
use otc_server::{
    api::{CreateSwapRequest, CreateSwapResponse},
    server::run_server,
    OtcServerArgs,
};
use reqwest::StatusCode;
use sqlx::{pool::PoolOptions, postgres::PgConnectOptions, types::chrono::Utc};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::info;
use uuid::Uuid;

use crate::utils::{
    build_bitcoin_wallet_descriptor, build_mm_test_args, build_otc_server_test_args,
    build_test_user_ethereum_wallet, build_tmp_bitcoin_wallet_db_file, get_free_port,
    wait_for_otc_server_to_be_ready, wait_for_swap_to_be_settled, PgConnectOptionsExt,
};

#[sqlx::test]
async fn test_swap_from_bitcoin_to_ethereum(
    _: PoolOptions<sqlx::Postgres>,
    connect_options: PgConnectOptions,
) {
    let market_maker_account = MultichainAccount::new(1);
    let user_account = MultichainAccount::new(2);

    let devnet = RiftDevnet::builder()
        .using_token_indexer(connect_options.to_database_url())
        .using_esplora(true)
        .build()
        .await
        .unwrap()
        .0;

    let mut wallet_join_set = JoinSet::new();

    let user_bitcoin_wallet = BitcoinWallet::new(
        &build_tmp_bitcoin_wallet_db_file(),
        &build_bitcoin_wallet_descriptor(&user_account.bitcoin_wallet.private_key),
        bitcoin::Network::Regtest,
        &devnet.bitcoin.esplora_url.as_ref().unwrap().to_string(),
        &mut wallet_join_set,
    )
    .await
    .unwrap();

    // fund all accounts

    devnet
        .bitcoin
        .deal_bitcoin(
            &user_account.bitcoin_wallet.address,
            &bitcoin::Amount::from_sat(500_000_000), // 5 BTC
        )
        .await
        .unwrap();
    devnet
        .ethereum
        .fund_eth_address(
            market_maker_account.ethereum_address,
            U256::from(100_000_000_000_000_000_000i128),
        )
        .await
        .unwrap();

    devnet
        .ethereum
        .mint_cbbtc(
            market_maker_account.ethereum_address,
            U256::from(9_000_000_000i128), // 90 cbbtc
        )
        .await
        .unwrap();

    let mut service_join_set = JoinSet::new();

    let otc_port = get_free_port().await;
    let otc_args = build_otc_server_test_args(otc_port, &devnet, &connect_options);

    service_join_set.spawn(async move {
        run_server(otc_args)
            .await
            .expect("OTC server should not crash");
    });

    tokio::select! {
        _ = wait_for_otc_server_to_be_ready(otc_port) => {
            info!("OTC server is ready");
        }
        _ = service_join_set.join_next() => {
            panic!("OTC server crashed");
        }
        _ = wallet_join_set.join_next() => {
            panic!("Bitcoin wallet crashed");
        }
    }

    let mm_args = build_mm_test_args(otc_port, &market_maker_account, &devnet);
    let mm_uuid = mm_args.market_maker_id.clone().parse::<Uuid>().unwrap();
    service_join_set.spawn(async move {
        run_market_maker(mm_args)
            .await
            .expect("Market maker should not crash");
    });
    devnet
        .bitcoin
        .wait_for_esplora_sync(Duration::from_secs(30))
        .await
        .unwrap();
    // at this point, the user should have a confirmed BTC balance
    // and our market maker should have plenty of cbbtc to fill their order
    // create a swap request

    let client = reqwest::Client::new();
    // create a swap request
    let swap_request = CreateSwapRequest {
        quote: Quote {
            id: Uuid::new_v4(),
            market_maker_id: mm_uuid,
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(10_000_000), // 0.1 BTC
                decimals: 8,
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Address(
                    devnet.ethereum.cbbtc_contract.address().to_string(),
                ),
                amount: U256::from(9_000_000), // 0.09 cbbtc
                decimals: 8,
            },
            expires_at: Utc::now() + Duration::from_secs(60 * 60 * 24),
            created_at: Utc::now(),
        },
        user_destination_address: user_account.ethereum_address.to_string(),
        user_refund_address: user_account.bitcoin_wallet.address.to_string(),
    };

    let response = client
        .post(format!("http://localhost:{otc_port}/api/v1/swaps"))
        .json(&swap_request)
        .send()
        .await
        .unwrap();

    let response_status = response.status();
    let response_json = match response_status {
        StatusCode::OK => {
            let response_json: CreateSwapResponse = response.json().await.unwrap();
            response_json
        }
        _ => {
            let response_text = response.text().await;
            panic!(
                "Swap request should be successful but got {response_status:#?} {response_text:#?}"
            );
            unreachable!()
        }
    };
    let tx_hash = user_bitcoin_wallet
        .create_transaction(
            &Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: response_json.expected_amount,
                decimals: response_json.decimals,
            },
            &response_json.deposit_address,
            None,
        )
        .await
        .unwrap();

    info!(
        "Broadcasting transaction from user wallet to deposit address: {}",
        tx_hash
    );
    devnet.bitcoin.mine_blocks(6).await.unwrap();
    info!("Mined block");

    let get_tx_status = devnet
        .bitcoin
        .rpc_client
        .get_raw_transaction_verbose(&tx_hash.parse::<bitcoin::Txid>().unwrap())
        .await
        .unwrap();

    info!("Tx status: {:#?}", get_tx_status);
    wait_for_swap_to_be_settled(otc_port, response_json.swap_id).await;

    drop(devnet);
    tokio::join!(wallet_join_set.shutdown(), service_join_set.shutdown());
}

#[sqlx::test]
async fn test_swap_from_ethereum_to_bitcoin(
    _: PoolOptions<sqlx::Postgres>,
    connect_options: PgConnectOptions,
) {
    let market_maker_account = MultichainAccount::new(1);
    let user_account = MultichainAccount::new(2);

    let devnet = RiftDevnet::builder()
        .using_token_indexer(connect_options.to_database_url())
        .bitcoin_mining_mode(MiningMode::Interval(2))
        .using_esplora(true)
        .build()
        .await
        .unwrap()
        .0;

    let (mut wallet_join_set, user_ethereum_wallet) =
        build_test_user_ethereum_wallet(&devnet, &user_account).await;

    // fund all accounts
    devnet
        .bitcoin
        .deal_bitcoin(
            &market_maker_account.bitcoin_wallet.address,
            &bitcoin::Amount::from_sat(500_000_000), // 5 BTC
        )
        .await
        .unwrap();

    devnet
        .ethereum
        .fund_eth_address(
            user_account.ethereum_address,
            U256::from(100_000_000_000_000_000_000i128),
        )
        .await
        .unwrap();

    devnet
        .ethereum
        .mint_cbbtc(
            user_account.ethereum_address,
            U256::from(9_000_000_000i128), // 90 cbbtc
        )
        .await
        .unwrap();

    let mut service_join_set = JoinSet::new();

    let otc_port = get_free_port().await;
    let otc_args = build_otc_server_test_args(otc_port, &devnet, &connect_options);

    service_join_set.spawn(async move {
        run_server(otc_args)
            .await
            .expect("OTC server should not crash");
    });

    tokio::select! {
        _ = wait_for_otc_server_to_be_ready(otc_port) => {
            info!("OTC server is ready");
        }
        _ = service_join_set.join_next() => {
            panic!("OTC server crashed");
        }
        _ = wallet_join_set.join_next() => {
            panic!("Bitcoin wallet crashed");
        }
    }

    let mm_args = build_mm_test_args(otc_port, &market_maker_account, &devnet);
    let mm_uuid = mm_args.market_maker_id.clone().parse::<Uuid>().unwrap();
    service_join_set.spawn(async move {
        run_market_maker(mm_args)
            .await
            .expect("Market maker should not crash");
    });
    devnet
        .bitcoin
        .wait_for_esplora_sync(Duration::from_secs(30))
        .await
        .unwrap();
    // at this point, the user should have a confirmed BTC balance
    // and our market maker should have plenty of cbbtc to fill their order
    // create a swap request

    let client = reqwest::Client::new();
    // create a swap request
    let swap_request = CreateSwapRequest {
        quote: Quote {
            id: Uuid::new_v4(),
            market_maker_id: mm_uuid,
            from: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Address(
                    devnet.ethereum.cbbtc_contract.address().to_string(),
                ),
                amount: U256::from(100_000_000i128), // 1 cbbtc
                decimals: 8,
            },
            to: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(90_000_000i128), //  0.9 BTC
                decimals: 8,
            },
            expires_at: Utc::now() + Duration::from_secs(60 * 60 * 24),
            created_at: Utc::now(),
        },
        user_destination_address: user_account.bitcoin_wallet.address.to_string(),
        user_refund_address: user_account.ethereum_address.to_string(),
    };

    let response = client
        .post(format!("http://localhost:{otc_port}/api/v1/swaps"))
        .json(&swap_request)
        .send()
        .await
        .unwrap();

    let response_status = response.status();
    let response_json = match response_status {
        StatusCode::OK => {
            let response_json: CreateSwapResponse = response.json().await.unwrap();
            response_json
        }
        _ => {
            let response_text = response.text().await;
            panic!(
                "Swap request should be successful but got {response_status:#?} {response_text:#?}"
            );
            unreachable!()
        }
    };
    let tx_hash = user_ethereum_wallet
        .create_transaction(
            &Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Address(
                    devnet.ethereum.cbbtc_contract.address().to_string(),
                ),
                amount: response_json.expected_amount,
                decimals: response_json.decimals,
            },
            &response_json.deposit_address,
            None,
        )
        .await
        .unwrap();

    info!(
        "Broadcasting transaction from user wallet to deposit address: {}",
        tx_hash
    );
    devnet
        .ethereum
        .funded_provider
        .anvil_mine(Some(2), None)
        .await
        .unwrap();
    info!("Mined 2 blocks");

    let get_tx_status = devnet
        .ethereum
        .funded_provider
        .get_transaction_receipt(tx_hash.parse::<TxHash>().unwrap())
        .await
        .unwrap();

    info!("Tx status: {:#?}", get_tx_status);
    wait_for_swap_to_be_settled(otc_port, response_json.swap_id).await;

    drop(devnet);
    tokio::join!(wallet_join_set.shutdown(), service_join_set.shutdown());
}
