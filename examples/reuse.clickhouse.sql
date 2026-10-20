SELECT *
FROM cases
WHERE raised > now() - INTERVAL 30 DAY AND assignee IS NULL
LIMIT 10
