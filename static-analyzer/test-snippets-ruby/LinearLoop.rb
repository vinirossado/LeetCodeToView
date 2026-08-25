def sum(arr)
  total = 0
  i = 0
  while i < arr.length
    total += arr[i]
    i += 1
  end
  total
end

puts sum([1, 2, 3, 4, 5])
