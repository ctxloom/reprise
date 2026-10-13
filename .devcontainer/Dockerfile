# Base image for both the VS Code devcontainer and the ctxloom agent image.
#
# `just` is installed HERE rather than via a devcontainer feature. ctxloom does
# not run the devcontainer CLI, so it does not honor `features:` — an image
# built from a feature-supplied `just` comes up without it, and every build,
# test and lint path in this repo goes through `just` (an ltk hook redirects
# raw `cargo`). Installing it in the Dockerfile is the one place both consumers
# read.
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Pinned to match the host toolchain (just 1.40.0) so the container and the dev
# box run the same recipes. Bump both together.
ARG JUST_VERSION=1.40.0
RUN curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
      | bash -s -- --tag "${JUST_VERSION}" --to /usr/local/bin \
    && just --version
