use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapStatus {
    QuoteValidation,      // Checking with MM
    QuoteRejected,        // MM won't fill
    WaitingUserDeposit,   // Wallets created, waiting for user
    WaitingMMDeposit,     // User deposited, notified MM
    WaitingConfirmations, // Both deposited, waiting for confirms
    Settling,             // Sending funds to user
    Completed,            // User withdrawal tx broadcast
    Refunding,            // Something went wrong, refunding user
}