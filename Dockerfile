# Build stage
FROM rust:alpine AS builder

# Install necessary build dependencies
# musl-dev is required for linking against musl libc
RUN apk add --no-cache musl-dev

WORKDIR /app

# Copy manifests first to cache dependencies
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
# This prevents re-downloading/building crates if only source code changes
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy the actual source code
COPY src ./src

# Build the application
# touch main.rs to ensure cargo rebuilds the app instead of using the cached dummy
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM alpine:latest

# Create a non-root user for security
RUN addgroup -S flicker && adduser -S flicker -G flicker

WORKDIR /app

# Copy the compiled binary from the builder stage
COPY --from=builder /app/target/release/flicker /usr/local/bin/flicker

# Use the non-root user
USER flicker

# Set the entrypoint
ENTRYPOINT ["flicker"]

