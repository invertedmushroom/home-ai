FROM node:24-bookworm-slim@sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c
LABEL deep.worker.protocol="node-v2"
COPY --chmod=0555 runner.cjs /opt/deep/runner.cjs
COPY --chmod=0555 outputs.cjs /opt/deep/outputs.cjs
USER 0:0
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/node", "/opt/deep/runner.cjs"]
