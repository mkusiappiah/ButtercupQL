//! ButtercupQL, a Python-like language for SQL, named after the buttercup flower.
//!
//! A query is written one step per line, in the order the data flows, and compiles to the SQL a
//! database already runs, with every value passed as a parameter. The language is described in
//! `docs/design.md`; the compiler is built from it once the design is approved.

use std::fmt;
use std::str::FromStr;

/// The version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The databases ButtercupQL compiles to, in the order they are being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// SQLite.
    Sqlite,
    /// PostgreSQL.
    Postgres,
    /// DuckDB.
    DuckDb,
    /// ClickHouse.
    ClickHouse,
}

impl Dialect {
    /// Every dialect, in the order they are being built.
    pub const ALL: [Dialect; 4] = [
        Dialect::Sqlite,
        Dialect::Postgres,
        Dialect::DuckDb,
        Dialect::ClickHouse,
    ];

    /// The dialect's name, as the command line and the Python package take it.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Sqlite => "sqlite",
            Dialect::Postgres => "postgres",
            Dialect::DuckDb => "duckdb",
            Dialect::ClickHouse => "clickhouse",
        }
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A dialect name that ButtercupQL does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownDialect(pub String);

impl fmt::Display for UnknownDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown dialect `{}`: use sqlite, postgres, duckdb or clickhouse", self.0)
    }
}

impl std::error::Error for UnknownDialect {}

impl FromStr for Dialect {
    type Err = UnknownDialect;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Ok(Dialect::Sqlite),
            "postgres" | "postgresql" => Ok(Dialect::Postgres),
            "duckdb" => Ok(Dialect::DuckDb),
            "clickhouse" => Ok(Dialect::ClickHouse),
            _ => Err(UnknownDialect(text.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_reads_back_from_its_name() {
        for dialect in Dialect::ALL {
            assert_eq!(dialect.name().parse::<Dialect>(), Ok(dialect));
            assert_eq!(dialect.to_string(), dialect.name());
        }
    }

    #[test]
    fn names_are_read_without_regard_to_case_and_postgresql_is_accepted() {
        assert_eq!("PostgreSQL".parse::<Dialect>(), Ok(Dialect::Postgres));
        assert_eq!(" DuckDB ".parse::<Dialect>(), Ok(Dialect::DuckDb));
    }

    #[test]
    fn an_unknown_dialect_says_which_ones_exist() {
        let error = "oracle".parse::<Dialect>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "unknown dialect `oracle`: use sqlite, postgres, duckdb or clickhouse"
        );
    }
}
