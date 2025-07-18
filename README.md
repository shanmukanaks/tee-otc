# OTC Settlement in a Trusted Exeuction Environment

Cross-chain OTC swaps secured by Trusted Execution Environments (TEEs).

## Key Features

- **Universal chain support** - Any blockchain with standard wallet primitives
- **Bidirectional BTC swaps** - Native Bitcoin swaps without wrapping
- **No smart contracts** - Pure wallet-based settlement
- **TEE-secured** - Runs in Intel TDX secure enclaves

## Components

- `otc-server` - Rust WebSocket server for wallet creation and handling the full swap lifecycle, runs in a TEE
- Bitcoin full node integration
- Helios light client for Ethereum verification
- TEE attestation and secure key management
