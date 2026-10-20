SELECT count(*) AS operators
FROM (
  SELECT operator, count(*) AS n
  FROM cases
  GROUP BY operator
  HAVING count(*) > 100
) AS _q1
