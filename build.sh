#!/bin/bash

#--- ARGS
NAME=fbielejec/mer-de-glace
VERSION=$(git log -1 --pretty=%h)

#--- FUNCTIONS

function build {
  {

    echo "============================================="
    echo  "Buidling: "$NAME:$VERSION""
    echo "============================================="

    cargo build --release
    docker build -t $NAME:$VERSION -f Dockerfile .

  } || {
    echo "EXCEPTION WHEN BUIDLING "$NAME""
    exit 1
  }

}

function push {
  echo "Pushing: " $IMAGE
  docker tag $NAME:$VERSION $NAME:latest
  docker push $NAME #:$VERSION
}

function login {
  echo "$DOCKER_PASSWORD" | docker login -u "$DOCKER_USERNAME" --password-stdin
}

#--- EXECUTE

login
build
push

exit $?
