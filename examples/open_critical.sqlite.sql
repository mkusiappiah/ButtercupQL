SELECT key, account, risk
FROM cases
WHERE status = ? AND band IN (?, ?) AND raised > datetime('now', '-7 days')
ORDER BY risk DESC
LIMIT 20
-- parameter 1: "open"
-- parameter 2: "critical"
-- parameter 3: "high"
