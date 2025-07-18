-- TEE-OTC Database Schema

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Enum for wallet types
DO $$ BEGIN
    CREATE TYPE wallet_type AS ENUM ('btc', 'eth');
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

-- Enum for swap status
DO $$ BEGIN
    CREATE TYPE swap_status AS ENUM ('pending', 'active', 'completed', 'cancelled');
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

-- Quotes table
CREATE TABLE IF NOT EXISTS quotes (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL,
    from_currency wallet_type NOT NULL,
    to_currency wallet_type NOT NULL,
    from_amount NUMERIC(20, 8) NOT NULL,
    to_amount NUMERIC(20, 8) NOT NULL,
    market_maker VARCHAR(255) NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Wallets table
CREATE TABLE IF NOT EXISTS wallets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    swap_id UUID,
    wallet_type wallet_type NOT NULL,
    address VARCHAR(255) NOT NULL,
    private_key_encrypted TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Swaps table
CREATE TABLE IF NOT EXISTS swaps (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    quote_id UUID NOT NULL REFERENCES quotes(id),
    user_wallet_id UUID REFERENCES wallets(id),
    mm_wallet_id UUID REFERENCES wallets(id),
    status swap_status NOT NULL DEFAULT 'pending',
    user_deposit_tx VARCHAR(255),
    mm_deposit_tx VARCHAR(255),
    settlement_block_height BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Add foreign key constraint if it doesn't exist
DO $$ BEGIN
    ALTER TABLE wallets ADD CONSTRAINT fk_wallet_swap 
        FOREIGN KEY (swap_id) REFERENCES swaps(id);
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

-- Create indexes if they don't exist
CREATE INDEX IF NOT EXISTS idx_quotes_user_id ON quotes(user_id);
CREATE INDEX IF NOT EXISTS idx_quotes_expires_at ON quotes(expires_at);
CREATE INDEX IF NOT EXISTS idx_swaps_status ON swaps(status);
CREATE INDEX IF NOT EXISTS idx_swaps_quote_id ON swaps(quote_id);
CREATE INDEX IF NOT EXISTS idx_wallets_swap_id ON wallets(swap_id);