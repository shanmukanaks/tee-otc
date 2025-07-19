# OTC Settlement in a Trusted Execution Environment

Cross-chain OTC swaps secured by Trusted Execution Environments (TEEs).

## Components

- `otc-server` - Rust WebSocket server for wallet creation and handling the full swap lifecycle, runs in a TEE
- Bitcoin full node integration
- Helios light client for Ethereum verification
- TEE attestation and secure key management

## Development Workflow

1. **Start PostgreSQL with automatic setup**:

   ```bash
   docker compose -f compose.test-db.yml up -d
   ```

   This will automatically:
   - Create the `otc_dev` database
   - Apply the schema (embedded in compose file)
   - Run in the background (-d for detached mode)
   
   To check logs: `docker compose -f compose.test-db.yml logs`
   To stop: `docker compose -f compose.test-db.yml down`

2. **Build the project**:

   ```bash
   cargo build
   ```

3. **Run tests**:
   ```bash
   cargo test
   ```

## Development

### Database Configuration

This project uses `sqlx` with compile-time query validation. The `DATABASE_URL` is configured in `.cargo/config.toml` for development. The database and schema are automatically set up when you run `docker compose -f compose.test-db.yml up`.
