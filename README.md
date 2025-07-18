# tee-otc

Cross-chain OTC swaps secured by Trusted Execution Environments (TEEs).

## Components

- `otc-server` - Rust WebSocket server for deposit wallet creation and handling the full swap lifecycle, runs in a TEE
- Bitcoin full node integration
- Helios light client for Ethereum verification
