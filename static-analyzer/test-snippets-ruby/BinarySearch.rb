def binary_search(array, target)
  left = 0
  right = array.length - 1

  while left <= right
    mid = left + (right - left) / 2

    if array[mid] == target
      return mid
    end

    if array[mid] < target
      left = mid + 1
    else
      right = mid - 1
    end
  end

  -1
end

puts binary_search([1, 2, 3, 4, 5], 4)
