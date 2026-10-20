SELECT key, account, risk
FROM cases
WHERE status = $1 AND band IN ($2, $3) AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC
LIMIT 20
-- parameter 1: "open"
-- parameter 2: "critical"
-- parameter 3: "high"
