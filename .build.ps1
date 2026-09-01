$env:CI = ""; Set-Location c:\Users\yxpil\Desktop\BIT; npx.cmd tauri build > build-log.txt 2>&1; "EXIT=$LASTEXITCODE" | Out-File .build-done.txt -Encoding utf8
