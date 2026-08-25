def duplicate(arr)
  copy = Array.new(arr.length)
  i = 0
  while i < arr.length
    copy[i] = arr[i]
    i += 1
  end
  copy
end

puts duplicate([1, 2, 3]).length
