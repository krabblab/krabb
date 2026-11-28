use super::check;
use crate::command::Dialect::{Cmd, PowerShell};
use crate::command::Kind::{Deletes, Installs, ReadOnly, Services, Unknown, Writes};
use crate::command::classify;

const HOST: &str = "the command reaches the network location `host`";
const WAIT: &str = "`get-content -wait` keeps running until it is stopped";

#[test]
fn cmd_commands_are_classified() {
    check(
        Cmd,
        &[
            ("dir", ReadOnly, "`dir` only reads"),
            ("dir /s /b src", ReadOnly, "`dir` only reads"),
            ("DIR", ReadOnly, "`dir` only reads"),
            ("type README.md", ReadOnly, "`type` only reads"),
            ("findstr /n main src\\main.rs", ReadOnly, "`findstr` only reads"),
            ("where cargo", ReadOnly, "`where` only reads"),
            ("ver", ReadOnly, "`ver` only reads"),
            ("type a.txt | findstr x", ReadOnly, "`findstr` only reads"),
            ("dir && ver", ReadOnly, "`ver` only reads"),
            ("dir & ver", ReadOnly, "`ver` only reads"),
            ("dir 2>nul", ReadOnly, "`dir` only reads"),
            ("dir >NUL 2>&1", ReadOnly, "`dir` only reads"),
            ("type \"a & del b\"", ReadOnly, "`type` only reads"),
            ("type < a.txt", ReadOnly, "`type` only reads"),
            ("git status", Unknown, "`git` can run programs the repository configures"),
            ("dir > out.txt", Writes, "the command writes its output to `out.txt`"),
            ("dir>>out.txt", Writes, "the command writes its output to `out.txt`"),
            ("copy a b", Writes, "`copy` changes files"),
            ("dir & del x", Deletes, "`del` deletes files"),
            ("rd /s /q x", Deletes, "`rd` deletes files"),
            ("del,x", Deletes, "the command runs `del,x` by path"),
            ("d^el x", Deletes, "the command uses an escape character"),
            ("where.exe x", Unknown, "the command runs `where.exe` by path"),
            ("findstr.bat x", Unknown, "the command runs `findstr.bat` by path"),
            ("\"dir\"", Unknown, "the program name `dir` is quoted"),
            ("winget install x", Installs, "`winget` installs packages"),
            ("winget list", Unknown, "`winget` looks up packages"),
            ("msiexec /i x.msi", Installs, "`msiexec` installs packages"),
            ("sc stop spooler", Services, "`sc` starts, stops, or changes services"),
            ("sc query spooler", Unknown, "`sc` looks up services"),
            ("net stop spooler", Services, "`net start` starts, stops, or changes services"),
            ("runas /user:admin \"del x\"", Deletes, "`runas` runs with admin rights"),
            ("echo %PATH%", Unknown, "the command expands an environment variable"),
            ("echo !x!", Unknown, "the command uses delayed expansion"),
            ("(dir)", Unknown, "the command has a parenthesized group"),
            ("dir\ndel x", Deletes, "the command has more than one line"),
            ("dir \"x", Unknown, "a quote is not closed"),
            ("dir >&x", Writes, "the command has an unusual redirection"),
            ("cmd /c del x", Deletes, "`del` deletes files"),
            ("cmd /c \"dir & del x\"", Deletes, "`cmd` runs arbitrary code"),
            ("powershell -Command Remove-Item x", Deletes, "`remove-item` deletes files"),
            (
                "type \\\\server\\share\\x",
                Unknown,
                "the command reaches the network location `server`",
            ),
            (
                "type \\\\?\\UNC\\server\\share",
                Unknown,
                "the command reaches the network location `server`",
            ),
            (
                "type \\\\.\\PhysicalDrive0",
                Unknown,
                "the command opens the device `\\\\.\\PhysicalDrive0`",
            ),
            ("type \\\\?\\C:\\x", Unknown, "the command reaches `\\\\?\\C:\\x` outside the workspace"),
            ("start x", Unknown, "`start` is not on the read-only list"),
        ],
    );

    assert!(classify("runas /user:admin dir", Cmd).elevated);
}

#[test]
fn powershell_commands_are_classified() {
    check(
        PowerShell,
        &[
            ("Get-ChildItem", ReadOnly, "`get-childitem` only reads"),
            ("get-childitem -Recurse -Filter *.rs", ReadOnly, "`get-childitem` only reads"),
            ("ls", ReadOnly, "`get-childitem` only reads"),
            ("dir src", ReadOnly, "`get-childitem` only reads"),
            ("Get-Content README.md -TotalCount 10", ReadOnly, "`get-content` only reads"),
            ("cat a.txt", ReadOnly, "`get-content` only reads"),
            ("Select-String -Path *.rs -Pattern main", ReadOnly, "`select-string` only reads"),
            ("Test-Path 'Program Files'", ReadOnly, "`test-path` only reads"),
            ("Get-ChildItem -Include *.rs", ReadOnly, "`get-childitem` only reads"),
            (
                "Get-Content a.txt | Select-String x | Sort-Object",
                ReadOnly,
                "`sort-object` only reads",
            ),
            ("Get-Location; Get-Date", ReadOnly, "`get-date` only reads"),
            ("ls 2>$null", ReadOnly, "`get-childitem` only reads"),
            ("ls *> $null", ReadOnly, "`get-childitem` only reads"),
            ("ls 2>&1 | Select-Object -First 3", ReadOnly, "`select-object` only reads"),
            ("Write-Output 'it''s'", ReadOnly, "`write-output` only reads"),
            ("echo \"say \"\"hi\"\"\"", ReadOnly, "`write-output` only reads"),
            ("findstr x a.txt", ReadOnly, "`findstr` only reads"),
            ("ls > out.txt", Writes, "the command writes its output to `out.txt`"),
            ("ls | Out-File out.txt", Writes, "`out-file` changes files"),
            ("New-Item x", Writes, "`new-item` changes files"),
            ("Remove-Item x", Deletes, "`remove-item` deletes files"),
            ("rm -Recurse -Force x", Deletes, "`remove-item` deletes files"),
            (
                "Microsoft.PowerShell.Management\\Remove-Item x",
                Deletes,
                "`remove-item` deletes files",
            ),
            ("'rm' x", Deletes, "the program name `rm` is quoted"),
            ("& rm x", Deletes, "the command uses the call operator `&`"),
            ("ls {rm x}", Deletes, "the command has an expression"),
            ("Install-Module x", Installs, "`powershell` installs packages"),
            ("Find-Module x", Unknown, "`powershell` looks up packages"),
            (
                "Start-Service spooler",
                Services,
                "`start-service` starts, stops, or changes services",
            ),
            ("Get-Service", Unknown, "`get-service` looks up services"),
            ("Start-Process notepad", Unknown, "`start-process` runs other programs"),
            ("Get-Content a -OutVariable x", Unknown, "`get-content` sets a variable"),
            ("ls $env:HOME", Unknown, "an argument comes from a variable"),
            ("ls \"$env:HOME\"", Unknown, "a double-quoted string expands variables"),
            ("ls $(rm x)", Deletes, "the command has an expression"),
            ("ls @args", Unknown, "the command has an expression"),
            ("ls `; rm x", Unknown, "the command uses an escape character"),
            ("ls # x", Unknown, "the command has a comment"),
            ("ls < x", Unknown, "the command uses a reserved operator"),
            ("ls --% x", Unknown, "the command uses `--%`"),
            ("ls \u{2013}Force", Unknown, "the command has a typographic quote or dash"),
            ("ls 'x", Unknown, "a quote is not closed"),
            ("iex 'rm x'", Deletes, "`iex` runs arbitrary code"),
            ("Invoke-WebRequest x", Unknown, "`invoke-webrequest` uses the network"),
            ("where x", Unknown, "`where` is not on the read-only list"),
            ("[IO.File]::Delete('x')", Unknown, "the command has an expression"),
            ("Get-Content Env:PATH", Unknown, "the command reaches the `env` drive"),
            ("Get-ChildItem HKLM:\\Software", Unknown, "the command reaches the `hklm` drive"),
            ("Get-ChildItem -Path:Cert:\\", Unknown, "the command reaches the `cert` drive"),
            ("Get-Item Variable:x", Unknown, "the command reaches the `variable` drive"),
            ("gci Registry::HKEY_USERS", Unknown, "the command reaches the `registry` drive"),
            (
                "Get-Content FileSystem::\\\\server\\share\\x",
                Unknown,
                "the command reaches the network location `server`",
            ),
            (
                "Get-Content \\\\server\\share\\x",
                Unknown,
                "the command reaches the network location `server`",
            ),
            ("pwsh -c Remove-Item x", Deletes, "`remove-item` deletes files"),
        ],
    );

    let verdict = classify("Start-Process pwsh -Verb RunAs", PowerShell);

    assert!(verdict.elevated);
    assert_eq!(verdict.reasons[0].to_string(), "`start-process` runs with admin rights");
    assert!(classify("sudo ls", PowerShell).elevated);
}

#[test]
fn windows_lists_switch_values_and_namespaces_are_checked() {
    check(
        Cmd,
        &[
            ("findstr /G:\\\\host\\s\\f x", Unknown, HOST),
            ("findstr /F:C:\\x y", Unknown, "the command reaches `C:\\x` outside the workspace"),
            ("findstr /D:..\\x y", Unknown, "the command reaches `..\\x` outside the workspace"),
            ("findstr /G:list.txt x", ReadOnly, "`findstr` only reads"),
            ("dir x,\\\\host\\s", Unknown, HOST),
            ("dir x;\\\\host\\s", Unknown, HOST),
            ("type C:\\Users\\me\\x", Unknown, "the command reaches `C:\\Users\\me\\x` outside the workspace"),
            ("type D:x", Unknown, "the command reaches `D:x` outside the workspace"),
            ("type \\Windows\\x", Unknown, "the command reaches `\\Windows\\x` outside the workspace"),
            ("type %USERPROFILE%\\x", Unknown, "the command expands an environment variable"),
            (
                "type \\\\?\\GLOBALROOT\\Device\\x",
                Unknown,
                "the command opens the device `\\\\?\\GLOBALROOT\\Device\\x`",
            ),
            (
                "type \\??\\UNC\\host\\s",
                Unknown,
                "the command opens the device `\\??\\UNC\\host\\s`",
            ),
        ],
    );

    check(
        PowerShell,
        &[
            ("gc a,\\\\host\\s\\f", Unknown, HOST),
            ("gci .,env:", Unknown, "the command reaches the `env` drive"),
            ("gc ~\\x", Unknown, "the command reaches `~\\x` outside the workspace"),
            ("gc -Path:C:\\x", Unknown, "the command reaches `C:\\x` outside the workspace"),
            ("Get-Content a -Wait", Unknown, WAIT),
            ("gc a -wai", Unknown, WAIT),
            ("gc a -Wait:$true", Unknown, WAIT),
            ("ls *> $null", ReadOnly, "`get-childitem` only reads"),
            ("ls 12> $null", Unknown, "the command has an unusual redirection"),
        ],
    );
}
