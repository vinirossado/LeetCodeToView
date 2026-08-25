def count_pairs(arr)
  count = 0
  i = 0
  while i < arr.length
    j = 0
    while j < arr.length
      count += 1 if arr[i] == arr[j]
      j += 1
    end
    i += 1
  end
  count
end

puts count_pairs([1, 2, 3])
