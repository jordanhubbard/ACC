# Service definitions for sparky (100.87.229.125)
# Loaded by Consul client agent on sparky.

services {
  name = "whisper-api"
  id   = "whisper-api-sparky"
  port = 8792
  tags = ["ml", "speech-to-text", "gpu"]
  meta {
    host  = "sparky"
    model = "whisper-large-v3"
  }
  check {
    http     = "http://127.0.0.1:8792/health"
    interval = "15s"
    timeout  = "3s"
  }
}

services {
  name = "clawfs"
  id   = "clawfs-sparky"
  port = 8791
  tags = ["storage", "wasm"]
  meta {
    host = "sparky"
    desc = "Content-addressed WASM module store"
  }
  check {
    http     = "http://127.0.0.1:8791/health"
    interval = "15s"
    timeout  = "3s"
  }
}

services {
  name = "usdagent"
  id   = "usdagent-sparky"
  port = 8000
  tags = ["ml", "3d", "usd"]
  meta {
    host = "sparky"
    desc = "LLM-backed USD 3D asset generator"
  }
  check {
    http     = "http://127.0.0.1:8000/health"
    interval = "30s"
    timeout  = "5s"
  }
}

services {
  name = "ollama"
  id   = "ollama-sparky"
  port = 11434
  tags = ["ml", "inference", "gpu"]
  meta {
    host = "sparky"
    desc = "Local LLM inference (GB10)"
  }
  check {
    http     = "http://127.0.0.1:11434/"
    interval = "15s"
    timeout  = "3s"
  }
}
