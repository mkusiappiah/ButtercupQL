-- The tables the playground's examples read. Names are checked against them, and `account_id` links each case to
-- its account.
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    operator TEXT NOT NULL,
    balance NUMERIC
);

CREATE TABLE cases (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL,
    account_id TEXT REFERENCES accounts (id),
    status TEXT NOT NULL,
    band TEXT,
    risk INTEGER,
    raised TIMESTAMP NOT NULL,
    assignee TEXT,
    notes TEXT,
    account TEXT,
    operator TEXT
);

CREATE TABLE transactions (
    id INTEGER PRIMARY KEY,
    account_id TEXT REFERENCES accounts (id),
    operator TEXT NOT NULL,
    amount INTEGER NOT NULL,
    time TIMESTAMP NOT NULL
);
