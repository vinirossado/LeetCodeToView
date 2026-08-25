# Ruby's actually idiomatic loop style (no equivalent test in the Java/C# batteries
# — those languages have no block-based iteration idiom). Exercises the `call` node
# + `block` field + RECOGNIZED_ITERATION_METHODS whitelist path in ruby_adapter.rs,
# not the `while`/`until`/`for` path the other snippets in this directory cover.
def sum_each(arr)
  total = 0
  arr.each do |x|
    total += x
  end
  total
end

puts sum_each([1, 2, 3, 4, 5])
