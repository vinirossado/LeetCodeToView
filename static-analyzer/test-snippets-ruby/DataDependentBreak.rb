def find_index(arr, target)
  result = -1
  i = 0
  while i < arr.length
    if arr[i] == target
      result = i
      break
    end
    i += 1
  end
  result
end

puts find_index([5, 3, 8, 1, 9], 8)
