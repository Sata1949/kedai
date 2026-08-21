@echo off
rem Run cargo with MSVC vcvars64 environment (for shells without MSVC env).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
cd /d C:\Users\LENOVO\Desktop\kedai\server-rs
%*
