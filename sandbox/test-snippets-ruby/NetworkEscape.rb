require "socket"

begin
  TCPSocket.new("example.com", 80)
  puts "connected"
rescue => e
  puts "blocked: #{e.class}"
end
