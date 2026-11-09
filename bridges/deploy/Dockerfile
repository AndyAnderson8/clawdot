# One image for both bridges. Build from the bridges/ directory:
#   docker build -f deploy/Dockerfile -t clawdot-bridges .
#   docker run -d --restart unless-stopped --env-file ntfy.env -v ./ntfy.toml:/config/bridge.toml:ro \
#       clawdot-bridges clawdot_bridges.ntfy
# The Discord library is installed only when asked for:  --build-arg WITH_DISCORD=1
FROM python:3.13-slim
ARG WITH_DISCORD=0
ENV PYTHONUNBUFFERED=1 PYTHONDONTWRITEBYTECODE=1
WORKDIR /app
COPY requirements-discord.txt ./
RUN if [ "$WITH_DISCORD" = "1" ]; then pip install --no-cache-dir -r requirements-discord.txt; fi
COPY clawdot_bridges ./clawdot_bridges
RUN useradd --system --no-create-home bridge
USER bridge
# The settings file is mounted read-only; secrets come from --env-file or the orchestrator, never the image.
ENTRYPOINT ["python", "-m"]
CMD ["clawdot_bridges.ntfy", "--config", "/config/bridge.toml"]
