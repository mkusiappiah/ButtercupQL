//! Tests that run the SQL ButtercupQL writes on real databases: SQLite always, and PostgreSQL when
//! `BCQL_POSTGRES_URL` names one. They live in `tests/`, so that the library itself needs no database drivers.
