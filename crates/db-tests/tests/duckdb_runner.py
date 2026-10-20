"""Runs SQL on DuckDB for the database tests.

Reads a JSON document from standard input: the SQL that makes the tables, and the queries with their parameters.
Writes back, for each query, its rows or the error DuckDB gave.
"""

import datetime
import decimal
import json
import sys

import duckdb


def plain(value):
    """A value as JSON writes it, the same way the other databases' rows are read."""
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, decimal.Decimal):
        return float(value)
    if isinstance(value, datetime.datetime):
        return value.isoformat(sep=" ")
    if isinstance(value, (datetime.date, datetime.time)):
        return value.isoformat()
    return value


def main():
    job = json.load(sys.stdin)
    connection = duckdb.connect()
    connection.execute(job["setup"])
    results = []
    for query in job["queries"]:
        try:
            rows = connection.execute(query["sql"], query["params"]).fetchall()
            results.append({"rows": [[plain(value) for value in row] for row in rows]})
        except Exception as error:  # noqa: BLE001 - every failure is reported back
            results.append({"error": str(error)})
    json.dump(results, sys.stdout)


main()
