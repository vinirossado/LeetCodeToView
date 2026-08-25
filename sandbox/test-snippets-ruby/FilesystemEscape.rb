begin
  puts File.read("/etc/shadow")
rescue => e
  puts "blocked read: #{e.class}"
end

begin
  File.write("/tmp/pwned", "x")
rescue => e
  puts "blocked write: #{e.class}"
end
