SELECT *
FROM cases
WHERE raised > now() - INTERVAL '30 days' AND assignee IS NULL
LIMIT 10
