def count_halvings(n)
  steps = 0
  while n > 1
    n = n / 2
    steps += 1
  end
  steps
end

puts count_halvings(1024)
