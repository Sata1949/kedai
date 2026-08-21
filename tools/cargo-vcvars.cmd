@echo off
rem 在 vcvars64 环境下执行 cargo 命令(供 Git Bash / 无 MSVC 环境的 shell 调用)
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
cd /d C:\Users\LENOVO\Desktop\kedai\server-rs
%*
