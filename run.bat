@echo off
REM =============================================================================
REM HYDRA-UMC-TWIN - run.bat
REM Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
REM GPL-3.0 - see LICENSE
REM =============================================================================
REM Forwards all arguments (e.g. "run.bat family-status").
setlocal
cd /d "%~dp0"

set BIN=target\release\hydra-umc-twin.exe

if not exist "%BIN%" (
    echo No compiled binary found - run build.bat first. 1>&2
    exit /b 1
)

"%BIN%" %*
set EXIT_CODE=%errorlevel%
pause
exit /b %EXIT_CODE%
