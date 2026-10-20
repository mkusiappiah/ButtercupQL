SELECT operator, key, risk, rank() OVER (PARTITION BY operator ORDER BY risk DESC) AS place
FROM cases
WHERE status = $1
QUALIFY rank() OVER (PARTITION BY operator ORDER BY risk DESC) <= 3
ORDER BY operator, place
-- parameter 1: "open"
