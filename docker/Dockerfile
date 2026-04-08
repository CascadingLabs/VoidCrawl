FROM python:3.11-slim

# Single apt layer + locale/doc stripping keeps the image dive-clean.
RUN apt-get update && apt-get install -y --no-install-recommends \
        chromium \
        chromium-sandbox \
        supervisor \
        curl \
        fonts-liberation \
        libnss3 \
        libatk-bridge2.0-0 \
        libdrm2 \
        libxcomposite1 \
        libxdamage1 \
        libxrandr2 \
        libgbm1 \
        libasound2 \
        libpangocairo-1.0-0 \
        libgtk-3-0 \
    && rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/* \
              /usr/share/doc/* /usr/share/man/* /usr/share/locale/* \
    && mkdir -p /tmp/chrome-profile-1 /tmp/chrome-profile-2

WORKDIR /app

# Supervisord config
COPY docker/config/supervisord.conf /etc/supervisor/conf.d/supervisord.conf

# Dynamic entrypoint — generates supervisord.conf via voidcrawl.scale at start
COPY docker/entrypoint.sh /docker/entrypoint.sh
RUN chmod +x /docker/entrypoint.sh

# Scale profile drives Chrome farm sizing (minimal | balanced | advanced)
ENV SCALE_PROFILE=balanced \
    TAB_MAX_USES=50 \
    CHROME_NO_SANDBOX=1

EXPOSE 9222 9223

HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:9222/json/version

ENTRYPOINT ["/docker/entrypoint.sh"]
