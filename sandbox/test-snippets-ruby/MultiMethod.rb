def helper(n)
  return 1 if n <= 1

  n * helper(n - 1)
end

result = helper(5)
puts result
