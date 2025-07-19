pub mod conversions;
pub mod quote_repo;
pub mod row_mappers;
pub mod swap_repo;
#[cfg(test)]
mod test_helpers;

pub use quote_repo::QuoteRepository;
pub use swap_repo::SwapRepository;

use snafu::Snafu;
use sqlx::postgres::PgPool;

#[derive(Debug, Snafu)]
pub enum DbError {
    #[snafu(display("Database query failed: {}", source))]
    Query { source: sqlx::Error },
    
    #[snafu(display("Record not found"))]
    NotFound,
    
    #[snafu(display("Invalid data format: {}", message))]
    InvalidData { message: String },
    
    #[snafu(display("Transaction failed: {}", source))]
    Transaction { source: sqlx::Error },
}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => DbError::NotFound,
            _ => DbError::Query { source: err },
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    
    pub fn quotes(&self) -> QuoteRepository {
        QuoteRepository::new(self.pool.clone())
    }
    
    pub fn swaps(&self) -> SwapRepository {
        SwapRepository::new(self.pool.clone())
    }
}