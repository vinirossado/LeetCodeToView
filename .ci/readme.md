# Deploy

## api (Quarkus + sandbox-runner + static-analyzer)

Runs OUTSIDE Docker Swarm's service model — a plain, systemd-managed
`docker run` container. Not the original design; changed after a real,
validated finding (see `.ci/leetcodeview-api.service`'s own header
comment, `tasks.md`, and `spec.md`'s "Isolamento de execução: nsjail"):
Swarm has no way to grant `cgroupns=host`/`security_opt` overrides, both
genuinely required for `nsjail` to isolate untrusted code on a real Linux
host — confirmed against a real Swarm cluster and a real (non-VM) Linux
kernel, not assumed.

One-time setup on the VPS:

```
ruby scripts/publish.rb
docker network create -d overlay --attachable proxy_net   # if it doesn't exist yet — MUST be --attachable
sudo cp .ci/leetcodeview-api.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now leetcodeview-api.service
```

Every deploy after that:

```
./deploy.sh
```

(pulls the latest image, `systemctl restart`s the unit, waits for the
real `/health` endpoint — see that file.)

## frontend

Still a normal Swarm service — it doesn't need any of the privileges
above.

```
ruby scripts/publish-fe.rb
docker stack deploy -c .ci/stack-fe.yml leetcodeview-fe
```
