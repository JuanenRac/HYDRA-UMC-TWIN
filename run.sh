#!/usr/bin/env bash
# HYDRA_UMC_SCRIPT_STANDARD_HEADER_BEGIN
# *****************************************************************************
# Project   : HYDRA-UMC-TWIN
# Script    : run.sh
# Purpose   : Runtime workflow for the project entry point.
# Author    : JuanenRac (Electro Hobby 3D)
# Email     : electrohobby3d@gmail.com
# Copyright : (C) 2026 JuanenRac
# License   : GPL-3.0 - see LICENSE
# *****************************************************************************
# HYDRA_UMC_SCRIPT_STANDARD_HEADER_END
# HYDRA_UMC_SCRIPT_STANDARD_BANNER_BEGIN
printf '\n*******************************************************************************\n'
printf '%s\n' "* HYDRA-UMC-TWIN - run.sh"
printf '%s\n' "* Mode      : RUN WORKFLOW"
printf '%s\n' "* Author    : JuanenRac (Electro Hobby 3D)"
printf '%s\n' "* Email     : electrohobby3d@gmail.com"
printf '%s\n' "* Copyright : (C) 2026 JuanenRac"
printf '%s\n' "* License   : GPL-3.0 - see LICENSE"
printf '%s\n' "* ------------------------------------------------------------------------- *"
printf '%s\n' "* 1. Resolve the runtime prerequisites declared by this script."
printf '%s\n' "* 2. Start the project entry point and forward user arguments unchanged."
printf '%s\n' "* 3. Preserve its result and keep an interactive terminal open."
printf '%s\n' "*******************************************************************************"
printf '\n'
# HYDRA_UMC_SCRIPT_STANDARD_BANNER_END

# HYDRA_UMC_SCRIPT_STANDARD_SAFE_PAUSE
# Prompt only in an interactive terminal: CI, pipes and service launchers never block.
hydra_umc_pause_on_exit() {
    local status=$?
    if [[ -t 0 && -t 1 ]]; then
        printf '\nPress Enter to close this window...'
        read -r _
    fi
    return "$status"
}
trap 'hydra_umc_pause_on_exit' EXIT

# Forwards all arguments (e.g. "./run.sh family-status").
set -euo pipefail
cd "$(dirname "$0")"

BIN="target/release/hydra-umc-twin"
if [ ! -f "$BIN" ] && [ ! -f "${BIN}.exe" ]; then
  echo "No compiled binary found - run build.sh first." >&2
  exit 1
fi

if [ -f "${BIN}.exe" ]; then
  exec "${BIN}.exe" "$@"
else
  exec "$BIN" "$@"
fi
