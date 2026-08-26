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

## Pi (ambiente de teste local, sem Swarm)

O Pi (`ssh alexcastrodev@pi`) é usado só pra validação local, não é o VPS
de produção (pizito) — não tem imagem nenhuma publicada no registry
`pizito:5001` acessível de lá, e não faz sentido pagar o custo de Swarm
(2 réplicas, rolling update) num ambiente de teste single-instance. Os
dois serviços rodam como containers `docker run` comuns gerenciados por
systemd, numa rede bridge simples — nenhum Swarm envolvido em nada.

One-time setup:

```
docker network create leetcodeview-net   # bridge comum, NÃO overlay
sudo cp .ci/leetcodeview-api.pi.service .ci/leetcodeview-fe.pi.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable leetcodeview-api.pi.service leetcodeview-fe.pi.service
```

Deploy (builda as duas imagens localmente, sem registry — ver
`.ci/deploy-pi.sh`):

```
git pull
./.ci/deploy-pi.sh
```

api fica em `localhost:8080`, frontend em `localhost:8081`.
