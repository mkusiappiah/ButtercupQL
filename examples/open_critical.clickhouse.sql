SELECT key, account, risk
FROM cases
WHERE status = {p1:String} AND band IN ({p2:String}, {p3:String}) AND raised > now() - INTERVAL 7 DAY
ORDER BY risk DESC
LIMIT 20
-- parameter 1: "open"
-- parameter 2: "critical"
-- parameter 3: "high"
