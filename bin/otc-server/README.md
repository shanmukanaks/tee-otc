# OTC Server

The main server for the TEE-OTC cross-chain swap system.

## Development

### Prerequisites

- Rust 1.70+
- PostgreSQL 14+
- Docker (optional, for running PostgreSQL)

### Database Setup

Start PostgreSQL with automatic database initialization:

```bash
docker compose -f compose.test-db.yml up
```

This will automatically:
- Start PostgreSQL on port 5432
- Create the `otc_dev` database
- Apply the schema from `src/db/schema.sql`
- Set up everything needed for development

The database will be ready when you see:
```
test_db | LOG:  database system is ready to accept connections
```

### Important: DATABASE_URL Configuration

The project uses `sqlx::query!` macros for compile-time SQL validation. This requires:

1. **`.cargo/config.toml`** - Contains the default DATABASE_URL for development
2. **Database must exist** - Run `./scripts/setup-dev-db.sh` before building
3. **For CI/CD** - Use `cargo sqlx prepare` to generate offline query data

### Running Tests

The database tests use `#[sqlx::test]` which automatically:
- Creates a temporary test database for each test
- Runs the schema setup
- Executes the test in isolation
- Drops the database after the test

To run tests:
```bash
# From the otc-server directory
cargo test

# Run a specific test
cargo test test_quote_round_trip

# Run with output
cargo test -- --nocapture
```

The `.env` file contains the template DATABASE_URL that sqlx::test uses to connect to PostgreSQL and create test databases.

### Running the Server

```bash
# Development mode
cargo run

# With custom settings
cargo run -- --host 0.0.0.0 --port 8080 --database-url postgres://user:pass@localhost/db
```

## Architecture

- `db/` - Database layer with repositories
  - `quote_repo.rs` - Quote CRUD operations
  - `swap_repo.rs` - Swap CRUD with public/private separation
  - `conversions.rs` - Type conversions for database storage
  - `row_mappers.rs` - Row to domain type mapping
  - `schema.sql` - Database schema

- `server.rs` - HTTP/WebSocket server setup
- `main.rs` - CLI entry point

## Testing

Tests are embedded in the repository files using `#[cfg(test)]` modules. This keeps tests close to the code they test and ensures they're updated together.

Key test scenarios:
- Round-trip serialization of all types
- U256 large number handling
- Public/Private data separation for swaps
- Enum serialization (ChainType, TokenIdentifier, SwapStatus)
- Timestamp precision handling