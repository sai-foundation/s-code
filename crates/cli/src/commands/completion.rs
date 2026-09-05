pub(crate) fn completion_script(shell: &str) -> &'static str {
    match shell {
        "bash" => {
            r#"_s_code() {
  local commands="exec review setup doctor sandbox mcp skill hook plugin app client completion"
  COMPREPLY=( $(compgen -W "$commands" -- "${COMP_WORDS[COMP_CWORD]}") )
}
complete -F _s_code s-code"#
        }
        "zsh" => {
            r#"#compdef s-code
_s_code() {
  local -a commands
  commands=('exec:run non-interactively' 'review:review Git changes' 'setup:configure the first model endpoint' 'doctor:diagnose setup' 'sandbox:run in the product sandbox' 'mcp:manage MCP servers' 'skill:manage Skills' 'hook:manage Hooks' 'plugin:manage Plugins and Marketplaces' 'app:list Plugin Apps' 'client:list or revoke connected clients' 'completion:generate shell completion')
  _describe 'command' commands
}
compdef _s_code s-code"#
        }
        "fish" => {
            r#"complete -c s-code -f
complete -c s-code -n '__fish_use_subcommand' -a exec -d 'Run non-interactively'
complete -c s-code -n '__fish_use_subcommand' -a review -d 'Review Git changes'
complete -c s-code -n '__fish_use_subcommand' -a setup -d 'Configure the first model endpoint'
complete -c s-code -n '__fish_use_subcommand' -a doctor -d 'Diagnose setup'
complete -c s-code -n '__fish_use_subcommand' -a sandbox -d 'Run in the product sandbox'
complete -c s-code -n '__fish_use_subcommand' -a mcp -d 'Manage MCP servers'
complete -c s-code -n '__fish_use_subcommand' -a skill -d 'Manage Skills'
complete -c s-code -n '__fish_use_subcommand' -a hook -d 'Manage Hooks'
complete -c s-code -n '__fish_use_subcommand' -a plugin -d 'Manage Plugins and Marketplaces'
complete -c s-code -n '__fish_use_subcommand' -a app -d 'List Plugin Apps'
complete -c s-code -n '__fish_use_subcommand' -a client -d 'List or revoke connected clients'
complete -c s-code -n '__fish_use_subcommand' -a completion -d 'Generate shell completion'"#
        }
        "powershell" => {
            r#"Register-ArgumentCompleter -Native -CommandName s-code -ScriptBlock {
  param($wordToComplete)
  'exec','review','setup','doctor','sandbox','mcp','skill','hook','plugin','app','client','completion' | Where-Object { $_ -like "$wordToComplete*" }
}"#
        }
        _ => unreachable!("shell is validated by parse_args"),
    }
}
