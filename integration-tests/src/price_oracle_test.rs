use tokio::task::JoinSet;

#[tokio::test]
async fn test_price_oracle() {
    let mut join_set = JoinSet::new();
    let price_oracle = market_maker::price_oracle::PriceOracle::new(&mut join_set);
    price_oracle.wait_for_connection().await.unwrap();

    let price = price_oracle.get_eth_per_btc().await;
    assert!(price.is_ok());
    println!("Price (ETH/BTC): {:?}", price.unwrap());

    join_set.abort_all();
}
