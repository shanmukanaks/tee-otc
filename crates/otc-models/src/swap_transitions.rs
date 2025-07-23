use crate::{Swap, SwapStatus, UserDepositStatus, MMDepositStatus, SettlementStatus};
use alloy::primitives::U256;
use chrono::Utc;
use snafu::{Snafu, ensure};

#[derive(Debug, Snafu)]
pub enum TransitionError {
    #[snafu(display("Invalid state transition from {:?} to {:?}", from, to))]
    InvalidTransition { from: SwapStatus, to: SwapStatus },
    
    #[snafu(display("Missing required data for transition: {}", reason))]
    MissingData { reason: String },
    
    #[snafu(display("Swap has already failed: {}", reason))]
    AlreadyFailed { reason: String },
}

pub type TransitionResult = Result<(), TransitionError>;

impl Swap {
    /// Transition to WaitingMMDeposit when user deposit is detected
    pub fn user_deposit_detected(
        &mut self,
        tx_hash: String,
        amount: U256,
        confirmations: u32,
    ) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::WaitingUserDeposit,
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::WaitingMMDeposit,
            }
        );
        
        let now = Utc::now();
        self.user_deposit_status = Some(UserDepositStatus {
            tx_hash,
            amount,
            detected_at: now,
            confirmations,
            last_checked: now,
        });
        
        self.status = SwapStatus::WaitingMMDeposit;
        self.updated_at = now;
        
        Ok(())
    }
    
    /// Transition to WaitingConfirmations when MM deposit is detected
    pub fn mm_deposit_detected(
        &mut self,
        tx_hash: String,
        amount: U256,
        confirmations: u32,
    ) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::WaitingMMDeposit,
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::WaitingConfirmations,
            }
        );
        
        let now = Utc::now();
        self.mm_deposit_status = Some(MMDepositStatus {
            tx_hash,
            amount,
            detected_at: now,
            confirmations,
            last_checked: now,
        });
        
        self.status = SwapStatus::WaitingConfirmations;
        self.updated_at = now;
        
        Ok(())
    }
    
    /// Update confirmation count for deposits
    pub fn update_confirmations(
        &mut self,
        user_confirmations: Option<u32>,
        mm_confirmations: Option<u32>,
    ) -> TransitionResult {
        let now = Utc::now();
        
        if let (Some(confirmations), Some(status)) = (user_confirmations, &mut self.user_deposit_status) {
            status.confirmations = confirmations;
            status.last_checked = now;
        }
        
        if let (Some(confirmations), Some(status)) = (mm_confirmations, &mut self.mm_deposit_status) {
            status.confirmations = confirmations;
            status.last_checked = now;
        }
        
        self.updated_at = now;
        Ok(())
    }
    
    /// Transition to Settling when confirmations are reached
    pub fn confirmations_reached(&mut self) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::WaitingConfirmations,
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::Settling,
            }
        );
        
        self.status = SwapStatus::Settling;
        self.updated_at = Utc::now();
        
        Ok(())
    }
    
    /// Record that MM was notified
    pub fn mark_mm_notified(&mut self) -> TransitionResult {
        self.mm_notified_at = Some(Utc::now());
        self.updated_at = Utc::now();
        Ok(())
    }
    
    /// Record that private key was sent to MM
    pub fn mark_private_key_sent(&mut self) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::Settling,
            MissingDataSnafu {
                reason: "Can only send private key during settling phase",
            }
        );
        
        self.mm_private_key_sent_at = Some(Utc::now());
        self.updated_at = Utc::now();
        Ok(())
    }
    
    /// Start settlement process
    pub fn settlement_initiated(&mut self, tx_hash: String) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::Settling,
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::Settling,
            }
        );
        
        let now = Utc::now();
        self.settlement_status = Some(SettlementStatus {
            tx_hash,
            broadcast_at: now,
            confirmations: 0,
            completed_at: None,
            fee: None,
        });
        
        self.updated_at = now;
        Ok(())
    }
    
    /// Complete the settlement
    pub fn settlement_completed(&mut self, confirmations: u32, fee: Option<U256>) -> TransitionResult {
        ensure!(
            self.status == SwapStatus::Settling,
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::Completed,
            }
        );
        
        let now = Utc::now();
        
        if let Some(settlement) = &mut self.settlement_status {
            settlement.confirmations = confirmations;
            settlement.completed_at = Some(now);
            settlement.fee = fee;
        } else {
            return Err(TransitionError::MissingData {
                reason: "Settlement status not found".to_string(),
            });
        }
        
        self.status = SwapStatus::Completed;
        self.updated_at = now;
        Ok(())
    }
    
    /// Initiate refund to user
    pub fn initiate_user_refund(&mut self, reason: String) -> TransitionResult {
        ensure!(
            matches!(
                self.status,
                SwapStatus::WaitingUserDeposit |
                SwapStatus::WaitingMMDeposit |
                SwapStatus::WaitingConfirmations
            ),
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::RefundingUser,
            }
        );
        
        self.status = SwapStatus::RefundingUser;
        self.failure_reason = Some(reason);
        self.updated_at = Utc::now();
        Ok(())
    }
    
    /// Initiate refund to both parties
    pub fn initiate_both_refunds(&mut self, reason: String) -> TransitionResult {
        ensure!(
            matches!(
                self.status,
                SwapStatus::WaitingConfirmations | SwapStatus::Settling
            ),
            InvalidTransitionSnafu {
                from: self.status,
                to: SwapStatus::RefundingBoth,
            }
        );
        
        self.status = SwapStatus::RefundingBoth;
        self.failure_reason = Some(reason);
        self.updated_at = Utc::now();
        Ok(())
    }
    
    /// Mark swap as failed
    pub fn mark_failed(&mut self, reason: String) -> TransitionResult {
        self.status = SwapStatus::Failed;
        self.failure_reason = Some(reason);
        self.updated_at = Utc::now();
        Ok(())
    }
    
    /// Check if swap has timed out
    pub fn is_timed_out(&self) -> bool {
        Utc::now() > self.timeout_at
    }
    
    /// Check if swap is in an active state (not completed or failed)
    pub fn is_active(&self) -> bool {
        !matches!(self.status, SwapStatus::Completed | SwapStatus::Failed)
    }
    
    /// Get required confirmations based on chain and amount
    pub fn get_required_confirmations(&self) -> (u32, u32) {
        // TODO: Implement logic based on chain type and amount
        // For now, return default values
        (3, 3) // (user_confirmations, mm_confirmations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use uuid::Uuid;
    
    fn create_test_swap() -> Swap {
        Swap {
            id: Uuid::new_v4(),
            quote_id: Uuid::new_v4(),
            market_maker: "test-mm".to_string(),
            user_deposit_salt: [0u8; 32],
            mm_deposit_salt: [0u8; 32],
            user_destination_address: "0x123".to_string(),
            user_refund_address: "bc1q123".to_string(),
            status: SwapStatus::WaitingUserDeposit,
            user_deposit_status: None,
            mm_deposit_status: None,
            settlement_status: None,
            failure_reason: None,
            timeout_at: Utc::now() + Duration::hours(1),
            mm_notified_at: None,
            mm_private_key_sent_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
    
    #[test]
    fn test_user_deposit_detected() {
        let mut swap = create_test_swap();
        
        // Valid transition
        swap.user_deposit_detected(
            "0xabc123".to_string(),
            U256::from(1000000u64),
            1,
        ).unwrap();
        
        assert_eq!(swap.status, SwapStatus::WaitingMMDeposit);
        assert!(swap.user_deposit_status.is_some());
        assert_eq!(swap.user_deposit_status.as_ref().unwrap().tx_hash, "0xabc123");
        
        // Invalid transition - can't deposit again
        let result = swap.user_deposit_detected(
            "0xdef456".to_string(),
            U256::from(1000000u64),
            1,
        );
        assert!(result.is_err());
    }
    
    #[test]
    fn test_full_happy_path() {
        let mut swap = create_test_swap();
        
        // User deposits
        swap.user_deposit_detected(
            "0xuser123".to_string(),
            U256::from(1000000u64),
            1,
        ).unwrap();
        
        // MM deposits
        swap.mm_deposit_detected(
            "0xmm456".to_string(),
            U256::from(500000u64),
            1,
        ).unwrap();
        
        // Confirmations reached
        swap.confirmations_reached().unwrap();
        assert_eq!(swap.status, SwapStatus::Settling);
        
        // Settlement initiated
        swap.settlement_initiated("0xsettle789".to_string()).unwrap();
        
        // Settlement completed
        swap.settlement_completed(6, Some(U256::from(1000u64))).unwrap();
        assert_eq!(swap.status, SwapStatus::Completed);
    }
    
    #[test]
    fn test_timeout_refund() {
        let mut swap = create_test_swap();
        swap.timeout_at = Utc::now() - Duration::hours(1); // Already timed out
        
        assert!(swap.is_timed_out());
        
        // Can refund user from waiting state
        swap.initiate_user_refund("Timeout waiting for user deposit".to_string()).unwrap();
        assert_eq!(swap.status, SwapStatus::RefundingUser);
    }
}