SELECT operator, key, risk, place
FROM (
  SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC) AS place
  FROM cases
  WHERE status = {p1:String}
) AS _q1
WHERE place <= 3
ORDER BY operator, place
-- parameter 1: "open"
