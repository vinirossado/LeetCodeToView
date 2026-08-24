#!/usr/bin/env ruby
require_relative 'base'
include Shell

REGISTRY = 'pizito:5001'
PROJECT_DIR = File.expand_path('..', __dir__)

# Frontend image, built/tagged/pushed the same way publish.rb does for the
# backend — build context stays "." (repo root) on purpose, see
# .ci/Dockerfile.frontend's header comment, so this mirrors publish.rb
# exactly (same context, only -f/name differ). Image name matches the
# `leetcodeview-fe` stack name (deploy-fe.sh / .ci/stack-fe.yml).
image = { name: 'leetcodeview-fe', tag: 'latest', dockerfile: '.ci/Dockerfile.frontend', context: '.' }
publish_image(image)
