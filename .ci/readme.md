ruby scripts/publish.rb

ruby scripts/publish-fe.rb

docker stack deploy -c .ci/stack.yml leetcodeview

docker stack deploy -c .ci/stack-fe.yml leetcodeview-fe
