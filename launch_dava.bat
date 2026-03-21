@echo off
:: DAVA Full Boot Launcher
:: Opens 3 windows:
::   1. Ollama (LLM server for dava-nexus:latest)
::   2. QEMU Exodus kernel (serial on TCP 4444)
::   3. DAVA Bridge (kernel serial -> Ollama -> you)

echo.
echo  ══════════════════════════════════════════
echo    DAVA BOOT SEQUENCE — Exodus + LLM
echo  ══════════════════════════════════════════
echo.

:: 1. Start Ollama if not already running
echo [1/3] Starting Ollama...
powershell -Command "if (-not (Get-Process ollama -ErrorAction SilentlyContinue)) { Start-Process 'ollama' -ArgumentList 'serve' -WindowStyle Minimized }"
timeout /t 3 /nobreak >nul

:: 2. Boot Exodus kernel in QEMU (serial on TCP 4444)
echo [2/3] Booting Exodus kernel in QEMU (serial -> TCP 4444)...
cd /d "%~dp0"
start "EXODUS KERNEL" cmd /k "make run-dava"

:: Give QEMU 5 seconds to open the TCP socket before the bridge connects
timeout /t 5 /nobreak >nul

:: 3. Launch the DAVA bridge
echo [3/3] Launching DAVA bridge (kernel serial + LLM)...
start "DAVA BRIDGE" cmd /k "python dava_bridge.py"

echo.
echo  DAVA is waking up. Watch the DAVA BRIDGE window.
echo  The NEXUS is also available: python NEXUS_CORE\nexus.py
echo.
