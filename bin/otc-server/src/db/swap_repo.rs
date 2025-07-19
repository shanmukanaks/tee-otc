use alloy::primitives::U256;
use otc_models::{SwapPrivate, SwapPublic, SwapSecrets, SwapStatus};
use sqlx::postgres::{PgPool, PgQueryResult};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::conversions::{chain_type_to_db, swap_status_to_db, u256_to_db};
use super::row_mappers::FromRow;
use super::{DbError, DbResult};

#[derive(Clone)]
pub struct SwapRepository {
    pool: PgPool,
}

impl SwapRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    
    pub async fn create(&self, swap: &SwapPrivate) -> DbResult<()> {
        let mut tx = self.pool.begin().await.map_err(|e| DbError::Transaction { source: e })?;
        
        // First, insert the swap public data
        self.insert_swap_public(&mut tx, &swap.public).await?;
        
        // Then, insert the swap secrets
        self.insert_swap_secrets(&mut tx, swap.public.id, &swap.secrets).await?;
        
        tx.commit().await.map_err(|e| DbError::Transaction { source: e })?;
        
        Ok(())
    }
    
    async fn insert_swap_public(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        swap: &SwapPublic,
    ) -> DbResult<PgQueryResult> {
        let user_deposit_chain = chain_type_to_db(&swap.user_deposit.chain);
        let mm_deposit_chain = chain_type_to_db(&swap.mm_deposit.chain);
        let status = swap_status_to_db(&swap.status);
        
        let result = sqlx::query(
            r#"
            INSERT INTO swaps (
                id, quote_id, market_maker,
                user_deposit_chain, user_deposit_address,
                mm_deposit_chain, mm_deposit_address,
                user_destination_address, user_refund_address,
                status,
                user_deposit_tx_hash, user_deposit_amount, user_deposit_detected_at,
                mm_deposit_tx_hash, mm_deposit_amount, mm_deposit_detected_at,
                user_withdrawal_tx,
                created_at, updated_at
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19
            )
            "#,
        )
        .bind(swap.id)
        .bind(swap.quote_id)
        .bind(&swap.market_maker)
        .bind(user_deposit_chain)
        .bind(&swap.user_deposit.address)
        .bind(mm_deposit_chain)
        .bind(&swap.mm_deposit.address)
        .bind(&swap.user_destination_address)
        .bind(&swap.user_refund_address)
        .bind(status)
        .bind(swap.user_deposit_status.as_ref().map(|d| d.tx_hash.clone()))
        .bind(swap.user_deposit_status.as_ref().map(|d| u256_to_db(&d.amount)))
        .bind(swap.user_deposit_status.as_ref().map(|d| d.detected_at))
        .bind(swap.mm_deposit_status.as_ref().map(|d| d.tx_hash.clone()))
        .bind(swap.mm_deposit_status.as_ref().map(|d| u256_to_db(&d.amount)))
        .bind(swap.mm_deposit_status.as_ref().map(|d| d.detected_at))
        .bind(swap.user_withdrawal_tx.clone())
        .bind(swap.created_at)
        .bind(swap.updated_at)
        .execute(&mut **tx)
        .await?;
        
        Ok(result)
    }
    
    async fn insert_swap_secrets(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        swap_id: Uuid,
        secrets: &SwapSecrets,
    ) -> DbResult<PgQueryResult> {
        let result = sqlx::query(
            r#"
            INSERT INTO swap_secrets (swap_id, user_deposit_private_key, mm_deposit_private_key)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(swap_id)
        .bind(&secrets.user_deposit_private_key)
        .bind(&secrets.mm_deposit_private_key)
        .execute(&mut **tx)
        .await?;
        
        Ok(result)
    }
    
    pub async fn get(&self, id: Uuid) -> DbResult<SwapPrivate> {
        let swap_row = sqlx::query(
            r#"
            SELECT 
                id, quote_id, market_maker,
                user_deposit_chain, user_deposit_address,
                mm_deposit_chain, mm_deposit_address,
                user_destination_address, user_refund_address,
                status,
                user_deposit_tx_hash, user_deposit_amount, user_deposit_detected_at,
                mm_deposit_tx_hash, mm_deposit_amount, mm_deposit_detected_at,
                user_withdrawal_tx,
                created_at, updated_at
            FROM swaps
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        
        let secrets_row = sqlx::query(
            r#"
            SELECT user_deposit_private_key, mm_deposit_private_key
            FROM swap_secrets
            WHERE swap_id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        
        let public = SwapPublic::from_row(&swap_row)?;
        let secrets = SwapSecrets::from_row(&secrets_row)?;
        
        Ok(SwapPrivate { public, secrets })
    }
    
    pub async fn get_public(&self, id: Uuid) -> DbResult<SwapPublic> {
        let row = sqlx::query(
            r#"
            SELECT 
                id, quote_id, market_maker,
                user_deposit_chain, user_deposit_address,
                mm_deposit_chain, mm_deposit_address,
                user_destination_address, user_refund_address,
                status,
                user_deposit_tx_hash, user_deposit_amount, user_deposit_detected_at,
                mm_deposit_tx_hash, mm_deposit_amount, mm_deposit_detected_at,
                user_withdrawal_tx,
                created_at, updated_at
            FROM swaps
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        
        SwapPublic::from_row(&row)
    }
    
    pub async fn update_status(&self, id: Uuid, status: SwapStatus) -> DbResult<()> {
        let status_str = swap_status_to_db(&status);
        
        sqlx::query(
            r#"
            UPDATE swaps
            SET status = $2, updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(status_str)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn update_user_deposit(
        &self,
        id: Uuid,
        tx_hash: String,
        amount: U256,
    ) -> DbResult<()> {
        sqlx::query(
            r#"
            UPDATE swaps
            SET 
                user_deposit_tx_hash = $2,
                user_deposit_amount = $3,
                user_deposit_detected_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .bind(u256_to_db(&amount))
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn update_mm_deposit(
        &self,
        id: Uuid,
        tx_hash: String,
        amount: U256,
    ) -> DbResult<()> {
        sqlx::query(
            r#"
            UPDATE swaps
            SET 
                mm_deposit_tx_hash = $2,
                mm_deposit_amount = $3,
                mm_deposit_detected_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .bind(u256_to_db(&amount))
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn update_withdrawal_tx(&self, id: Uuid, tx_hash: String) -> DbResult<()> {
        sqlx::query(
            r#"
            UPDATE swaps
            SET 
                user_withdrawal_tx = $2,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn get_active_swaps(&self) -> DbResult<Vec<SwapPublic>> {
        let rows = sqlx::query(
            r#"
            SELECT 
                id, quote_id, market_maker,
                user_deposit_chain, user_deposit_address,
                mm_deposit_chain, mm_deposit_address,
                user_destination_address, user_refund_address,
                status,
                user_deposit_tx_hash, user_deposit_amount, user_deposit_detected_at,
                mm_deposit_tx_hash, mm_deposit_amount, mm_deposit_detected_at,
                user_withdrawal_tx,
                created_at, updated_at
            FROM swaps
            WHERE status NOT IN ('completed', 'quote_rejected', 'refunding')
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        
        let mut swaps = Vec::new();
        for row in rows {
            swaps.push(SwapPublic::from_row(&row)?);
        }
        
        Ok(swaps)
    }
    
    pub async fn get_swaps_by_market_maker(&self, mm_identifier: &str) -> DbResult<Vec<SwapPublic>> {
        let rows = sqlx::query(
            r#"
            SELECT 
                id, quote_id, market_maker,
                user_deposit_chain, user_deposit_address,
                mm_deposit_chain, mm_deposit_address,
                user_destination_address, user_refund_address,
                status,
                user_deposit_tx_hash, user_deposit_amount, user_deposit_detected_at,
                mm_deposit_tx_hash, mm_deposit_amount, mm_deposit_detected_at,
                user_withdrawal_tx,
                created_at, updated_at
            FROM swaps
            WHERE market_maker = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(mm_identifier)
        .fetch_all(&self.pool)
        .await?;
        
        let mut swaps = Vec::new();
        for row in rows {
            swaps.push(SwapPublic::from_row(&row)?);
        }
        
        Ok(swaps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use chrono::{Duration, Utc};
    use otc_models::{
        ChainType, Currency, DepositInfo, Quote, SwapPrivate, SwapPublic, SwapSecrets, SwapStatus,
        TokenIdentifier, WalletPublic,
    };
    use crate::db::Database;
    use uuid::Uuid;

    #[sqlx::test]
    async fn test_swap_round_trip(pool: sqlx::PgPool) -> sqlx::Result<()> {
        // Initialize database schema
        crate::db::test_helpers::test_helpers::setup_test_schema(&pool).await?;

        let db = Database::new(pool.clone());
        let swap_repo = db.swaps();

        // First create a quote that the swap will reference
        let quote = Quote {
            id: Uuid::new_v4(),
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(1000000u64), // 0.01 BTC
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Native,
                amount: U256::from(500000000000000000u64), // 0.5 ETH
            },
            market_maker_identifier: "test-mm-1".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            created_at: Utc::now(),
        };
        
        // Insert the quote
        db.quotes().create(&quote).await.unwrap();

        // Create a test swap
        let original_swap = SwapPrivate {
            public: SwapPublic {
                id: Uuid::new_v4(),
                quote_id: quote.id,
                market_maker: "test-mm-1".to_string(),
                user_deposit: WalletPublic {
                    chain: ChainType::Bitcoin,
                    address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
                },
                mm_deposit: WalletPublic {
                    chain: ChainType::Ethereum,
                    address: "0x742d35Cc6634C0532925a3b844Bc9e7595f2Bd7e".to_string(),
                },
                user_destination_address: "0x1234567890123456789012345678901234567890".to_string(),
                user_refund_address: "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
                status: SwapStatus::WaitingUserDeposit,
                user_deposit_status: None,
                mm_deposit_status: None,
                user_withdrawal_tx: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            secrets: SwapSecrets {
                user_deposit_private_key: "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3ncxMMA3".to_string(),
                mm_deposit_private_key: "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318".to_string(),
            },
        };

        // Store the swap
        swap_repo.create(&original_swap).await.unwrap();

        // Retrieve the swap (with secrets)
        let retrieved_swap = swap_repo.get(original_swap.public.id).await.unwrap();

        // Validate public data
        assert_eq!(retrieved_swap.public.id, original_swap.public.id);
        assert_eq!(retrieved_swap.public.quote_id, original_swap.public.quote_id);
        assert_eq!(retrieved_swap.public.market_maker, original_swap.public.market_maker);
        assert_eq!(retrieved_swap.public.user_destination_address, original_swap.public.user_destination_address);
        assert_eq!(retrieved_swap.public.user_refund_address, original_swap.public.user_refund_address);
        assert_eq!(retrieved_swap.public.status, original_swap.public.status);

        // Validate wallet data
        assert_eq!(retrieved_swap.public.user_deposit.chain, original_swap.public.user_deposit.chain);
        assert_eq!(retrieved_swap.public.user_deposit.address, original_swap.public.user_deposit.address);
        assert_eq!(retrieved_swap.public.mm_deposit.chain, original_swap.public.mm_deposit.chain);
        assert_eq!(retrieved_swap.public.mm_deposit.address, original_swap.public.mm_deposit.address);

        // Validate secrets
        assert_eq!(retrieved_swap.secrets.user_deposit_private_key, original_swap.secrets.user_deposit_private_key);
        assert_eq!(retrieved_swap.secrets.mm_deposit_private_key, original_swap.secrets.mm_deposit_private_key);

        Ok(())
    }

    #[sqlx::test]
    async fn test_swap_with_deposits(pool: sqlx::PgPool) -> sqlx::Result<()> {
        // Initialize database schema
        crate::db::test_helpers::test_helpers::setup_test_schema(&pool).await?;

        let db = Database::new(pool.clone());
        let swap_repo = db.swaps();

        // Create quote
        let quote = Quote {
            id: Uuid::new_v4(),
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(2000000u64),
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Native,
                amount: U256::from(1000000000000000000u64),
            },
            market_maker_identifier: "test-mm-2".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            created_at: Utc::now(),
        };
        db.quotes().create(&quote).await.unwrap();

        // Create swap with deposit info
        let now = Utc::now();
        let original_swap = SwapPrivate {
            public: SwapPublic {
                id: Uuid::new_v4(),
                quote_id: quote.id,
                market_maker: "test-mm-2".to_string(),
                user_deposit: WalletPublic {
                    chain: ChainType::Bitcoin,
                    address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".to_string(),
                },
                mm_deposit: WalletPublic {
                    chain: ChainType::Ethereum,
                    address: "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(),
                },
                user_destination_address: "0x9876543210987654321098765432109876543210".to_string(),
                user_refund_address: "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3".to_string(),
                status: SwapStatus::WaitingConfirmations,
                user_deposit_status: Some(DepositInfo {
                    tx_hash: "7d865e959b2466918c9863afca942d0fb89d7c9ac0c99bafc3749504ded97730".to_string(),
                    amount: U256::from(2000000u64),
                    detected_at: now,
                }),
                mm_deposit_status: Some(DepositInfo {
                    tx_hash: "0x88df016429689c079f3b2f6ad39fa052532c56b6a39df8e3c84c03b8346cfc63".to_string(),
                    amount: U256::from(1000000000000000000u64),
                    detected_at: now + Duration::minutes(5),
                }),
                user_withdrawal_tx: None,
                created_at: now,
                updated_at: now + Duration::minutes(5),
            },
            secrets: SwapSecrets {
                user_deposit_private_key: "cN8kTVqrJLJHMQQQLsEEVMqB5YFA4wZ2TXv3sfQvhFdRQ8a7DBRG".to_string(),
                mm_deposit_private_key: "0x2c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318".to_string(),
            },
        };

        // Store and retrieve
        swap_repo.create(&original_swap).await.unwrap();
        let retrieved_swap = swap_repo.get(original_swap.public.id).await.unwrap();

        // Validate deposit info
        assert!(retrieved_swap.public.user_deposit_status.is_some());
        let user_deposit = retrieved_swap.public.user_deposit_status.unwrap();
        let original_user_deposit = original_swap.public.user_deposit_status.unwrap();
        assert_eq!(user_deposit.tx_hash, original_user_deposit.tx_hash);
        assert_eq!(user_deposit.amount, original_user_deposit.amount);
        assert!((user_deposit.detected_at - original_user_deposit.detected_at).num_seconds().abs() < 1);

        assert!(retrieved_swap.public.mm_deposit_status.is_some());
        let mm_deposit = retrieved_swap.public.mm_deposit_status.unwrap();
        let original_mm_deposit = original_swap.public.mm_deposit_status.unwrap();
        assert_eq!(mm_deposit.tx_hash, original_mm_deposit.tx_hash);
        assert_eq!(mm_deposit.amount, original_mm_deposit.amount);
        assert!((mm_deposit.detected_at - original_mm_deposit.detected_at).num_seconds().abs() < 1);

        Ok(())
    }

    #[sqlx::test]
    async fn test_swap_status_updates(pool: sqlx::PgPool) -> sqlx::Result<()> {
        // Initialize database schema
        crate::db::test_helpers::test_helpers::setup_test_schema(&pool).await?;

        let db = Database::new(pool.clone());
        let swap_repo = db.swaps();

        // Create quote
        let quote = Quote {
            id: Uuid::new_v4(),
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(1000000u64),
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Native,
                amount: U256::from(500000000000000000u64),
            },
            market_maker_identifier: "test-mm-3".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            created_at: Utc::now(),
        };
        db.quotes().create(&quote).await.unwrap();

        // Create swap
        let swap = SwapPrivate {
            public: SwapPublic {
                id: Uuid::new_v4(),
                quote_id: quote.id,
                market_maker: "test-mm-3".to_string(),
                user_deposit: WalletPublic {
                    chain: ChainType::Bitcoin,
                    address: "bc1qnahvmnz8vgsdmrr68l5mfr8v8q9fxqz3n5d9u0".to_string(),
                },
                mm_deposit: WalletPublic {
                    chain: ChainType::Ethereum,
                    address: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
                },
                user_destination_address: "0xabcdef1234567890abcdef1234567890abcdef12".to_string(),
                user_refund_address: "bc1qnahvmnz8vgsdmrr68l5mfr8v8q9fxqz3n5d9u0".to_string(),
                status: SwapStatus::QuoteValidation,
                user_deposit_status: None,
                mm_deposit_status: None,
                user_withdrawal_tx: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            secrets: SwapSecrets {
                user_deposit_private_key: "cMahea7zHyW3yMWoeN4jTK2Cbu1RjCFdMA6wfBRYWEfvBXYWHKLX".to_string(),
                mm_deposit_private_key: "0x1c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318".to_string(),
            },
        };

        swap_repo.create(&swap).await.unwrap();

        // Update status
        swap_repo.update_status(swap.public.id, SwapStatus::WaitingUserDeposit).await.unwrap();
        
        let updated = swap_repo.get_public(swap.public.id).await.unwrap();
        assert_eq!(updated.status, SwapStatus::WaitingUserDeposit);

        // Update user deposit
        let deposit_amount = U256::from(1000000u64);
        swap_repo.update_user_deposit(
            swap.public.id,
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
            deposit_amount,
        ).await.unwrap();

        let updated = swap_repo.get_public(swap.public.id).await.unwrap();
        assert!(updated.user_deposit_status.is_some());
        let deposit = updated.user_deposit_status.unwrap();
        assert_eq!(deposit.amount, deposit_amount);

        // Update withdrawal tx
        swap_repo.update_withdrawal_tx(
            swap.public.id,
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_string(),
        ).await.unwrap();

        let updated = swap_repo.get_public(swap.public.id).await.unwrap();
        assert!(updated.user_withdrawal_tx.is_some());
        assert_eq!(
            updated.user_withdrawal_tx.unwrap(),
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        );

        Ok(())
    }

    #[sqlx::test]
    async fn test_get_active_swaps(pool: sqlx::PgPool) -> sqlx::Result<()> {
        // Initialize database schema
        crate::db::test_helpers::test_helpers::setup_test_schema(&pool).await?;

        let db = Database::new(pool.clone());
        let swap_repo = db.swaps();

        // Create quote
        let quote = Quote {
            id: Uuid::new_v4(),
            from: Currency {
                chain: ChainType::Bitcoin,
                token: TokenIdentifier::Native,
                amount: U256::from(1000000u64),
            },
            to: Currency {
                chain: ChainType::Ethereum,
                token: TokenIdentifier::Native,
                amount: U256::from(500000000000000000u64),
            },
            market_maker_identifier: "test-mm-4".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
            created_at: Utc::now(),
        };
        db.quotes().create(&quote).await.unwrap();

        // Create multiple swaps with different statuses
        let statuses = vec![
            SwapStatus::WaitingUserDeposit,  // Active
            SwapStatus::WaitingMMDeposit,     // Active
            SwapStatus::WaitingConfirmations, // Active
            SwapStatus::Settling,             // Active
            SwapStatus::Completed,            // Not active
            SwapStatus::QuoteRejected,        // Not active
            SwapStatus::Refunding,            // Not active
        ];

        let mut swap_ids = Vec::new();
        for (i, status) in statuses.iter().enumerate() {
            let swap = SwapPrivate {
                public: SwapPublic {
                    id: Uuid::new_v4(),
                    quote_id: quote.id,
                    market_maker: "test-mm-4".to_string(),
                    user_deposit: WalletPublic {
                        chain: ChainType::Bitcoin,
                        address: format!("bc1q{:064}", i),
                    },
                    mm_deposit: WalletPublic {
                        chain: ChainType::Ethereum,
                        address: format!("0x{:040}", i),
                    },
                    user_destination_address: format!("0x{:040}", i + 100),
                    user_refund_address: format!("bc1q{:064}", i + 100),
                    status: status.clone(),
                    user_deposit_status: None,
                    mm_deposit_status: None,
                    user_withdrawal_tx: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                },
                secrets: SwapSecrets {
                    user_deposit_private_key: format!("privkey{}", i),
                    mm_deposit_private_key: format!("0xprivkey{:064}", i),
                },
            };

            swap_repo.create(&swap).await.unwrap();
            swap_ids.push((swap.public.id, status.clone()));
        }

        // Get active swaps
        let active_swaps = swap_repo.get_active_swaps().await.unwrap();

        // Should return only the first 4 swaps (active statuses)
        assert_eq!(active_swaps.len(), 4);

        // Verify only active statuses are returned
        for swap in &active_swaps {
            match swap.status {
                SwapStatus::WaitingUserDeposit |
                SwapStatus::WaitingMMDeposit |
                SwapStatus::WaitingConfirmations |
                SwapStatus::Settling => {
                    // These are expected active statuses
                }
                _ => panic!("Unexpected status in active swaps: {:?}", swap.status),
            }
        }

        Ok(())
    }
}