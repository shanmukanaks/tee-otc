use alloy::sol;

pub const DISPERSE_DEPLOYED_BYTECODE: &str = include_str!("disperse-deployed-code.hex");

sol! {
    #[sol(rpc)]
    Disperse,"src/disperse-abi.json",
}
