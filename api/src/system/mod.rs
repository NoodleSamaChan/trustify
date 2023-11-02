use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbErr, Statement,
    TransactionTrait,
};
use sea_orm_migration::MigratorTrait;

use migration::Migrator;
pub use vex::VexSystem;

mod error;
mod package;
mod sbom;
mod vex;

mod vulnerability;

#[derive(Clone)]
pub struct System {
    db: Arc<DatabaseConnection>,
}

pub struct Context<'t> {
    tx: &'t DatabaseTransaction,
}

impl<'t> Context<'t> {
    pub fn vex(&self) -> VexSystem {
        VexSystem { tx: self.tx }
    }
}

pub enum Error<E: Send> {
    Database(DbErr),
    Transaction(E),
}

impl<E: Send> From<DbErr> for Error<E> {
    fn from(value: DbErr) -> Self {
        Self::Database(value)
    }
}

impl<E: Send> Debug for Error<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transaction(_) => f.debug_tuple("Transaction").finish(),
            Self::Database(err) => f.debug_tuple("Database").field(err).finish(),
        }
    }
}

impl<E: Send + Display> std::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transaction(inner) => write!(f, "transaction error: {}", inner),
            Self::Database(err) => write!(f, "database error: {err}"),
        }
    }
}

impl<E: Send + Display> std::error::Error for Error<E> {}

impl System {
    pub async fn new(
        username: &str,
        password: &str,
        host: &str,
        db_name: &str,
    ) -> Result<Self, anyhow::Error> {
        let url = format!("postgres://{}:{}@{}/{}", username, password, host, db_name);
        println!("connect to {}", url);
        let db = Database::connect(url).await?;

        Migrator::refresh(&db).await?;

        Ok(Self { db: Arc::new(db) })
    }

    #[cfg(test)]
    pub async fn for_test(name: &str) -> Result<Self, anyhow::Error> {
        Self::bootstrap("postgres", "eggs", "localhost", name).await
    }

    pub async fn bootstrap(
        username: &str,
        password: &str,
        host: &str,
        db_name: &str,
    ) -> Result<Self, anyhow::Error> {
        let url = format!("postgres://{}:{}@{}/postgres", username, password, host);
        println!("bootstrap to {}", url);
        let db = Database::connect(url).await?;

        let drop_db_result = db
            .execute(Statement::from_string(
                db.get_database_backend(),
                format!("DROP DATABASE IF EXISTS \"{}\";", db_name),
            ))
            .await?;

        println!("{:?}", drop_db_result);

        let create_db_result = db
            .execute(Statement::from_string(
                db.get_database_backend(),
                format!("CREATE DATABASE \"{}\";", db_name),
            ))
            .await?;

        println!("{:?}", create_db_result);

        db.close().await?;

        Self::new(username, password, host, db_name).await
    }

    pub async fn transaction<F, T, E>(&self, f: F) -> Result<T, Error<E>>
    where
        F: for<'c> FnOnce(Context<'c>) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: Send,
    {
        let db = self.db.clone();
        let tx = db.begin().await?;

        let ctx = Context { tx: &tx };
        match f(ctx).await {
            Err(err) => {
                tx.rollback().await?;
                Err(Error::Transaction(err))
            }
            Ok(r) => {
                tx.commit().await?;
                Ok(r)
            }
        }
    }
}
