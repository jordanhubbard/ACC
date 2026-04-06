# Service definitions for boris (Sweden GPU fleet)
# Loaded by Consul client agent on boris.

services {
  name = "boris-vllm"
  id   = "boris-vllm-boris"
  port = 18080
  tags = ["ml", "inference", "gpu", "vllm"]
  meta {
    host  = "boris"
    model = "nemotron-120b-fp8"
    gpus  = "4xL40"
    desc  = "Nemotron-120B FP8 via vLLM"
  }
  check {
    http     = "http://127.0.0.1:18080/v1/models"
    interval = "30s"
    timeout  = "5s"
  }
}
