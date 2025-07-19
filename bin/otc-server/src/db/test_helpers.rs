#[cfg(test)]
pub mod test_helpers {
    use sqlx::PgPool;
    
    pub async fn setup_test_schema(pool: &PgPool) -> sqlx::Result<()> {
        // Create extension
        sqlx::query("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"")
            .execute(pool)
            .await?;
        
        // Create enum type
        sqlx::query(r#"
            DO $$ BEGIN
                CREATE TYPE swap_status AS ENUM (
                    'quote_validation',
                    'quote_rejected',
                    'waiting_user_deposit',
                    'waiting_mm_deposit',
                    'waiting_confirmations',
                    'settling',
                    'completed',
                    'refunding'
                );
            EXCEPTION
                WHEN duplicate_object THEN null;
            END $$
        "#)
        .execute(pool)
        .await?;
        
        // Create quotes table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS quotes (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                
                -- From currency details
                from_chain VARCHAR(50) NOT NULL,
                from_token JSONB NOT NULL,
                from_amount TEXT NOT NULL, -- U256 stored as string
                
                -- To currency details
                to_chain VARCHAR(50) NOT NULL,
                to_token JSONB NOT NULL,
                to_amount TEXT NOT NULL, -- U256 stored as string
                
                market_maker_identifier VARCHAR(255) NOT NULL,
                expires_at TIMESTAMPTZ NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
        "#)
        .execute(pool)
        .await?;
        
        // Create swaps table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS swaps (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                quote_id UUID NOT NULL REFERENCES quotes(id),
                market_maker VARCHAR(255) NOT NULL,
                
                -- User deposit wallet
                user_deposit_chain VARCHAR(50) NOT NULL,
                user_deposit_address VARCHAR(255) NOT NULL,
                
                -- MM deposit wallet
                mm_deposit_chain VARCHAR(50) NOT NULL,
                mm_deposit_address VARCHAR(255) NOT NULL,
                
                -- User addresses
                user_destination_address VARCHAR(255) NOT NULL,
                user_refund_address VARCHAR(255) NOT NULL,
                
                -- Status
                status VARCHAR(50) NOT NULL,
                
                -- Deposit tracking
                user_deposit_tx_hash VARCHAR(255),
                user_deposit_amount TEXT, -- U256 stored as string
                user_deposit_detected_at TIMESTAMPTZ,
                
                mm_deposit_tx_hash VARCHAR(255),
                mm_deposit_amount TEXT, -- U256 stored as string
                mm_deposit_detected_at TIMESTAMPTZ,
                
                -- Withdrawal tracking
                user_withdrawal_tx VARCHAR(255),
                
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
        "#)
        .execute(pool)
        .await?;
        
        // Create swap secrets table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS swap_secrets (
                swap_id UUID PRIMARY KEY REFERENCES swaps(id),
                user_deposit_private_key TEXT NOT NULL,
                mm_deposit_private_key TEXT NOT NULL
            )
        "#)
        .execute(pool)
        .await?;
        
        // Create indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_quotes_market_maker ON quotes(market_maker_identifier)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_quotes_expires_at ON quotes(expires_at)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swaps_status ON swaps(status)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swaps_quote_id ON swaps(quote_id)")
            .execute(pool)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_swaps_market_maker ON swaps(market_maker)")
            .execute(pool)
            .await?;
        
        Ok(())
    }
}