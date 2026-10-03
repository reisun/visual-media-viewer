FROM rust:1.95-trixie

RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
    libavdevice-dev libswscale-dev libswresample-dev libasound2-dev \
    clang libclang-dev pkg-config

WORKDIR /app
CMD ["bash", "scripts/test-video.sh"]
