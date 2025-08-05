use alloy::primitives::U256;
use market_maker::run_market_maker;
use otc_models::{ChainType, Currency, TokenIdentifier};
use rfq_server::server::run_server as run_rfq_server;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::utils::{
    build_mm_test_args, build_rfq_server_test_args, get_free_port, wait_for_rfq_server_to_be_ready,
    INTEGRATION_TEST_TIMEOUT_SECS, TEST_MARKET_MAKER_ID,
};

#[tokio::test]
async fn test_rfq_flow() {
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
    let mm_args = build_mm_test_args(otc_port, rfq_port, &market_maker_account, &devnet);
    join_set.spawn(async move {
        run_market_maker(mm_args)
            .await
            .expect("Market maker should not crash");
    });

    // Wait for market maker to connect to RFQ server
    let connected_url = format!("http://127.0.0.1:{rfq_port}/api/v1/market-makers/connected");

    let client = reqwest::Client::new();
    let start_time = std::time::Instant::now();
    let timeout = Duration::from_secs(INTEGRATION_TEST_TIMEOUT_SECS);

    loop {
        assert!(
            (start_time.elapsed() <= timeout),
            "Timeout waiting for market maker to connect to RFQ server"
        );

        tokio::time::sleep(Duration::from_millis(100)).await;

        if let Ok(response) = client.get(&connected_url).send().await {
            if response.status() == 200 {
                if let Ok(body) = response.json::<serde_json::Value>().await {
                    if let Some(market_makers) = body["market_makers"].as_array() {
                        if market_makers.len() == 1
                            && market_makers[0].as_str() == Some(TEST_MARKET_MAKER_ID)
                        {
                            println!("Market maker is connected to RFQ server!");
                            break;
                        }
                    }
                }
            }
        }
    }

    let from_amount = U256::from(100000000u64);
    let expected_to_amount = U256::from(100000000u64);

    // Now send a quote request
    let quote_request = rfq_server::server::QuoteRequest {
        from: Currency {
            chain: ChainType::Bitcoin,
            token: TokenIdentifier::Native,
            amount: from_amount, // 1 BTC
            decimals: 8,
        },
        to: Currency {
            chain: ChainType::Ethereum,
            token: TokenIdentifier::Native,
            amount: U256::ZERO, // Will be filled by MM
            decimals: 18,
        },
    };

    let quote_request_url = format!("http://127.0.0.1:{rfq_port}/api/v1/quotes/request");
    let response = client
        .post(&quote_request_url)
        .json(&quote_request)
        .send()
        .await
        .expect("Should be able to send quote request");

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
    assert_eq!(
        quote.market_maker_id.to_string(),
        TEST_MARKET_MAKER_ID,
        "Quote should be from our test market maker"
    );

    // Verify the amounts (MM currently returns symmetric quote)
    assert_eq!(
        quote.from.amount, from_amount,
        "From amount should match request"
    );
    assert_eq!(
        quote.to.amount, expected_to_amount,
        "To amount should be symmetric (for now)"
    );

    println!("RFQ flow test completed successfully!");
}
