FROM debian:stretch-slim
MAINTAINER "Filip Bielejec" <fbielejec@gmail.com>

RUN apt-get update && apt-get install -y \
    mysql-client libssl-dev ca-certificates \
    && rm -rf /tmp/* /var/{tmp,cache}/* /var/lib/{apt,dpkg}/

WORKDIR mer_de_glace

COPY target/release/mer-de-glace /mer_de_glace/mer-de-glace

ENTRYPOINT ["./mer-de-glace"]
