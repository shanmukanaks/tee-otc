use alloy::primitives::U256;
use market_maker::run_market_maker;
use otc_models::{ChainType, Currency, Lot, QuoteRequest, TokenIdentifier};
use otc_rfq_protocol::RFQResult;
use rfq_server::server::run_server as run_rfq_server;
use sqlx::{pool::PoolOptions, postgres::PgConnectOptions};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

use crate::utils::{
    build_mm_test_args, build_rfq_server_test_args, get_free_port,
    wait_for_market_maker_to_connect_to_rfq_server, wait_for_rfq_server_to_be_ready,
    INTEGRATION_TEST_TIMEOUT_SECS, TEST_MARKET_MAKER_ID,
};

#[sqlx::test]
async fn test_rfq_flow(_: PoolOptions<sqlx::Postgres>, connect_options: PgConnectOptions) {
    // Setup market maker account
    let market_maker_account = devnet::MultichainAccount::new(0);
    let devnet = devnet::RiftDevnet::builder()
        .using_esplora(true)
        .build()
        .await
        .unwrap()
        .0;

    let mut join_set = JoinSet::new();

    // Get free ports for RFQ server and dummy OTC server port
    let rfq_port = get_free_port().await;
    let otc_port = get_free_port().await; // Not used but needed for MM args

    tracing::info!("RFQ port: {}", rfq_port);
    tracing::info!("OTC port: {}", otc_port);

    // Start RFQ server
    let rfq_args = build_rfq_server_test_args(rfq_port);
    join_set.spawn(async move {
        run_rfq_server(rfq_args)
            .await
            .expect("RFQ server should not crash");
    });

    // Wait for RFQ server to be ready
    wait_for_rfq_server_to_be_ready(rfq_port).await;

    // Start market maker
    let mm_args = build_mm_test_args(
        otc_port,
        rfq_port,
        &market_maker_account,
        &devnet,
        &connect_options,
    )
    .await;
    join_set.spawn(async move {
        run_market_maker(mm_args)
            .await
            .expect("Market maker should not crash");
    });

    // Wait for market maker to connect to RFQ server
    wait_for_market_maker_to_connect_to_rfq_server(rfq_port).await;

    let from_amount = U256::from(100000000u64);

    // Now send a quote request
    let quote_request = QuoteRequest {
        mode: otc_models::QuoteMode::ExactOutput,
        amount: from_amount,
        from: Currency {
            chain: ChainType::Ethereum,
            token: TokenIdentifier::Address(devnet.ethereum.cbbtc_contract.address().to_string()),
            decimals: 8,
        },
        to: Currency {
            chain: ChainType::Bitcoin,
            token: TokenIdentifier::Native,
            decimals: 8,
        },
    };

    let quote_request_url = format!("http://127.0.0.1:{rfq_port}/api/v1/quotes/request");
    let client = reqwest::Client::new();

    // Start timing the quote request
    let start_time = Instant::now();

    let response = client
        .post(&quote_request_url)
        .json(&quote_request)
        .send()
        .await
        .expect("Should be able to send quote request");

    // Calculate and record the latency
    let latency = start_time.elapsed();

    assert_eq!(response.status(), 200, "Quote request should succeed");

    let quote_response: rfq_server::server::QuoteResponse = response
        .json()
        .await
        .expect("Should be able to parse quote response");

    // Verify the response
    assert_eq!(
        quote_response.total_quotes_received, 1,
        "Should receive 1 quote"
    );
    assert_eq!(
        quote_response.market_makers_contacted, 1,
        "Should contact 1 market maker"
    );

    // Verify the quote details
    let quote = &quote_response.quote;
    println!("Quote: {:?}", quote);

    assert!(quote.is_some(), "Quote should be present");
    match quote.as_ref().unwrap() {
        RFQResult::Success(quote) => {
            assert_eq!(
                quote.quote.market_maker_id.to_string(),
                TEST_MARKET_MAKER_ID,
                "Quote should be from our test market maker"
            );
        }
        _ => panic!("Quote should be a success"),
    }

    // Output the latency to get the quote
    tracing::info!("Quote request latency: {:?}", latency);

    println!("RFQ flow test completed successfully!");
}
