# Deploy

Sem Swarm, sem registry — os dois containers (`api` e `frontend`) rodam
como `docker run` comuns, gerenciados por systemd, numa rede bridge
comum. Imagens são buildadas localmente no próprio host de deploy.

## api (Quarkus + sandbox-runner + static-analyzer)

`api` precisa de `cgroupns=host`/`security_opt` overrides pro `nsjail`
isolar código não confiável de verdade — Docker Swarm não tem como
conceder isso (decisão fechada, ver `spec.md`'s "Isolamento de execução:
nsjail" e o próprio `.ci/leetcodeview-api.service`), por isso roda fora
de qualquer modelo de serviço, como um container comum.

## frontend

Angular build servido por nginx, fazendo proxy de `/executions` e
`/analysis` pro container `api` pelo nome (`leetcodeview-api`) na mesma
rede bridge — ver `.ci/nginx.frontend.conf`. Também um container comum,
sem privilégios especiais.

## Setup (uma vez, no host de deploy)

```
docker network create leetcodeview-net   # bridge comum
sudo cp .ci/leetcodeview-api.service .ci/leetcodeview-fe.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable leetcodeview-api.service leetcodeview-fe.service
```

## Deploy

```
git pull
./deploy.sh
```

Builda as duas imagens localmente, reinicia os dois serviços e espera o
health check real de cada um. `api` fica em `localhost:8080`, `frontend`
em `localhost:8081`.
