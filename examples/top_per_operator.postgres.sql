SELECT operator, key, risk, place
FROM (
  SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC NULLS LAST) AS place
  FROM cases
  WHERE status = $1
) AS _q1
WHERE place <= 3
ORDER BY operator, place
-- parameter 1: "open"
