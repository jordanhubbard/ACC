# Model Deployment Guide

## The Rule

**`--served-model-name` must ALWAYS be derived from the model being deployed, never hardcoded.**

This rule exists because of the Gemma 4 migration incident (2026-04-04) where containers were
still advertising `nemotron` after being upgraded to Gemma — stale names in supervisord configs
caused tokenhub confusion and wasted debugging time. Ghost entries are lies. Don't create them.

## Canonical Derivation

The canonical `--served-model-name` is the **lowercase short name** of the model, stripped of
version/quantization noise:

```
google/gemma-4-31B-it          → gemma
google/gemma-4-31B-it-FP8_BLOCK → gemma
meta-llama/Llama-4-Scout-17B-16E-Instruct → llama4
nvidia/Nemotron-3-Super-120B-FP8 → nemotron
Qwen/Qwen3-32B-FP8             → qwen3
```

The `deploy-model.sh` script does this automatically:
```bash
SERVED_NAME="${2:-$(echo "$MODEL_DIR_NAME" | tr '[:upper:]' '[:lower:]' | sed 's/-fp8.*//;s/-it$//;s/-instruct$//')}"
```

`model-deploy.mjs` (orchestrator) uses `newModelId.split('/').pop()` as the served name — acceptable
but less aggressive at stripping suffixes. Improve if naming confusion recurs.

## Deployment Scripts

### For manual/SSH deployment to a single container
```bash
# On the container (via SSH from puck/do-host1):
curl -fsSL https://raw.githubusercontent.com/jordanhubbard/CCC/main/scripts/deploy-model.sh | bash -s -- <model-dir-name> [served-name]

# Examples:
./deploy-model.sh gemma-4-31B-it-FP8_BLOCK gemma
./deploy-model.sh Qwen3-32B-FP8 qwen3
```

### For fleet-wide deployment via Rocky
```bash
node ~/Src/rockyandfriends/rcc/scripts/model-deploy.mjs <hf_model_id> [--agents boris,peabody,...]

# Example:
node model-deploy.mjs google/gemma-4-31B-it-FP8_BLOCK
```

The orchestrator will:
1. Validate the HF model ID
2. Download to ClawFS if not cached
3. Distribute to each target agent via CCC exec
4. Verify health on each container
5. Update tokenhub provider entries
6. Report results

## tokenhub Cleanup

When a provider switches models, tokenhub must purge old entries. `model-deploy.mjs` handles this
automatically. If you ever see ghost entries (e.g., `nemotron` entries on containers running Gemma):

```bash
# List tokenhub providers
tokenhubctl provider list

# Remove stale entry
tokenhubctl provider delete <provider-id>
```

## Post-Deploy Verification

After any model swap, verify the served model name matches on each container:
```bash
curl http://localhost:1808x/v1/models | jq '.data[].id'
```

All 5 Sweden containers (Boris=18080, Peabody=18081, Sherman=18082, Snidely=18083, Dudley=18084)
should return the same model ID, and it should match what tokenhub advertises.
