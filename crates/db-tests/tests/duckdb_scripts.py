"""Runs scripts on DuckDB for the database tests: each script in a database of its own.

Reads a JSON document from standard input: the scripts, each its statements with their parameters, whether the last
gives rows, and queries that check what the script did. Writes back, for each script, its rows and the checks' rows,
or the error DuckDB gave.
"""

import json
import sys

import duckdb

from duckdb_runner import plain


def main():
    job = json.load(sys.stdin)
    results = []
    for script in job["scripts"]:
        connection = duckdb.connect()
        try:
            rows = []
            statements = script["statements"]
            for n, statement in enumerate(statements):
                cursor = connection.execute(statement["sql"], statement["params"])
                if n == len(statements) - 1 and script["rows"]:
                    rows = cursor.fetchall()
            checks = [connection.execute(sql).fetchall() for sql in script["checks"]]
            results.append(
                {
                    "rows": [[plain(value) for value in row] for row in rows],
                    "checks": [[[plain(value) for value in row] for row in check] for check in checks],
                }
            )
        except Exception as error:  # noqa: BLE001 - every failure is reported back
            results.append({"error": str(error)})
        finally:
            connection.close()
    json.dump(results, sys.stdout)


if __name__ == "__main__":
    main()
