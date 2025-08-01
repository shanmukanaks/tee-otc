use alloy::{primitives::U256, providers::Provider};
use devnet::{MultichainAccount, RiftDevnet};
use evm_token_indexer_client::TokenIndexerClient;
use market_maker::{run_market_maker, MarketMakerArgs};
use otc_models::{ChainType, Currency, Quote, TokenIdentifier};
use otc_server::{api::CreateSwapRequest, server::run_server, OtcServerArgs};
use sqlx::{pool::PoolOptions, postgres::PgConnectOptions, types::chrono::Utc};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::info;
use uuid::Uuid;

use crate::utils::{
    build_mm_test_args, build_otc_server_test_args, get_free_port, wait_for_otc_server_to_be_ready,
    PgConnectOptionsExt,
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

    let mut join_set = JoinSet::new();
    let otc_port = get_free_port().await;
    let otc_args = build_otc_server_test_args(otc_port, &devnet, &connect_options);

    join_set.spawn(async move {
        run_server(otc_args)
            .await
            .expect("OTC server should not crash");
    });

    tokio::select! {
        _ = wait_for_otc_server_to_be_ready(otc_port) => {
            info!("OTC server is ready");
        }
        _ = join_set.join_next() => {
            panic!("OTC server crashed");
        }
    }

    let mm_args = build_mm_test_args(otc_port, &market_maker_account, &devnet);
    let mm_uuid = mm_args.market_maker_id.clone().parse::<Uuid>().unwrap();
    join_set.spawn(async move {
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

    /*
    assert_eq!(
        response.status(),
        200,
        "Swap request should be successful but got {response:#?}",
    );
    info!("response: {:#?}", response);
    */
}
