SELECT *
FROM cases
WHERE raised > datetime('now', '-30 days') AND assignee IS NULL
LIMIT 10
