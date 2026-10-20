SELECT operator, count(*) AS total, coalesce(sum(amount), 0) AS money, countIf(amount > 1000000) AS big
FROM transactions
WHERE time > now() - INTERVAL 30 DAY
GROUP BY operator
HAVING coalesce(sum(amount), 0) > 1000000
ORDER BY money DESC
