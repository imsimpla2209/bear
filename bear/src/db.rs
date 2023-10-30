use std::str::FromStr;
use sqlx::{Execute, FromRow, Pool, Sqlite, Transaction};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};

pub type DbTxn<'a> = Transaction<'a, Sqlite>;
pub type DbReadTxn<'a> = Transaction<'a, Sqlite>;
// TODO cant use write txn as read txn
pub type DbWriteTxn<'a> = Transaction<'a, Sqlite>;

#[derive(Clone)]
pub struct DbMain {
    readers: Pool<Sqlite>,
    writer: Pool<Sqlite>
}

impl DbMain {
    pub fn new(r: Pool<Sqlite>, w: Pool<Sqlite>) -> DbMain {
        DbMain {
            readers: r,
            writer: w
        }
    }

    pub async fn newtx_read(&self) -> anyhow::Result<DbReadTxn<'static>> {
        Ok(self.readers.begin().await?)
    }

    pub async fn newtx_write(&self) -> anyhow::Result<DbWriteTxn<'static>> {
        Ok(self.writer.begin().await?)
    }
}

pub async fn db_init(url: &str, migrator: &sqlx::migrate::Migrator) -> anyhow::Result<DbMain> {
    let opts = SqliteConnectOptions::from_str(url)?
        //.busy_timeout(Duration::from_secs(11))
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true);

    let wpool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts.clone()).await?;

    migrator.run(&wpool).await?;

    let rpool = SqlitePoolOptions::new()
        .connect_with(opts).await?;

    // NOTES
    // We are using WAL mode (which is default). In this mode there can be many readers with one writer at a time.
    // sqlx is planning to support W-only and R-only pools where the W-only pool has size 1. Unfortunately,
    // this is not available yet. This may be more efficient than having a timeout.
    // Sqlx has a separate thread pool that queries are dispatched to (apparently). So even running a single thread
    // code will log queries from multiple threads / (connections??)
    // The limit on one writer at a time applies to a whole transaction actually.

    Ok(DbMain::new(rpool, wpool))
}


pub trait TableMetadata {
    fn table_name() -> &'static str;
}

pub async fn find_opt_field<'a, 'f, T, F>(db: &mut DbTxn<'_>, f: &str, v: &'f F) -> anyhow::Result<Option<T>>
    where T: TableMetadata + for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
          F: Sync + Send + sqlx::Encode<'f, Sqlite> + sqlx::Type<Sqlite>
{
    let table = T::table_name();
    let querystr = format!("SELECT * FROM {table} WHERE {f} = ?");

    // this is how we construct arguments ...
    let mut tmp = sqlx::query("").bind(v);

    Ok(sqlx::query_as_with(&querystr, tmp.take_arguments().unwrap())
        .fetch_optional(&mut **db).await?)
}

pub async fn update_field<'a, 'f, T, F>(db: &mut DbTxn<'_>, f: &str, v: &'f F, id: &'f str) -> anyhow::Result<()>
    where T: TableMetadata + for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
          F: Sync + Send + sqlx::Encode<'f, Sqlite> + sqlx::Type<Sqlite>
{
    let table = T::table_name();
    let querystr = format!("UPDATE {table} SET {f} = ? WHERE id = ?");

    let mut tmp = sqlx::query("")
        .bind(v)
        .bind(id);

    sqlx::query_with(&querystr, tmp.take_arguments().unwrap())
        .execute(&mut **db)
        .await?;

    Ok(())
}

// Typed IDs
#[macro_export]
macro_rules! typed_id {
    ($x:ident) => {
        #[derive(sqlx::Type, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Eq, Hash, Default)]
        #[sqlx(transparent)]
        pub struct $x(pub String);

        impl From<String> for $x {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $x {
            fn from(s: &str) -> Self {
                Self(String::from(s))
            }
        }

        impl AsRef<str> for $x {
            fn as_ref(&self) -> &str {
                self.0.as_str()
            }
        }

        impl ToString for $x {
            fn to_string(&self) -> String {
                self.0.clone()
            }
        }

        impl ts_rs::TS for $x {
            fn name() -> String {
                String::from("string")
            }

            fn dependencies() -> Vec<ts_rs::Dependency> {
                Vec::new()
            }

            fn transparent() -> bool { false }
        }
    };
}

#[macro_export]
macro_rules! row_reader {
    ($cls:ident of [ $($x:ident),+ ]) => {
        impl<'r> FromRow<'r, SqliteRow> for $cls {
            fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
                Ok(Self {
                    $(
                        $x: row.try_get(stringify!($x))?,
                    )*
                })
            }
        }
    };
}