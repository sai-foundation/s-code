pub(crate) fn completion_script(shell: &str) -> &'static str {
    match shell {
        "bash" => {
            r#"_opencoding() {
  local commands="exec review doctor sandbox mcp skill hook plugin app client completion"
  COMPREPLY=( $(compgen -W "$commands" -- "${COMP_WORDS[COMP_CWORD]}") )
}
complete -F _opencoding opencoding"#
        }
        "zsh" => {
            r#"#compdef opencoding
_opencoding() {
  local -a commands
  commands=('exec:run non-interactively' 'review:review Git changes' 'doctor:diagnose setup' 'sandbox:run in the product sandbox' 'mcp:manage MCP servers' 'skill:manage Skills' 'hook:manage Hooks' 'plugin:manage Plugins and Marketplaces' 'app:list Plugin Apps' 'client:list or revoke connected clients' 'completion:generate shell completion')
  _describe 'command' commands
}
compdef _opencoding opencoding"#
        }
        "fish" => {
            r#"complete -c opencoding -f
complete -c opencoding -n '__fish_use_subcommand' -a exec -d 'Run non-interactively'
complete -c opencoding -n '__fish_use_subcommand' -a review -d 'Review Git changes'
complete -c opencoding -n '__fish_use_subcommand' -a doctor -d 'Diagnose setup'
complete -c opencoding -n '__fish_use_subcommand' -a sandbox -d 'Run in the product sandbox'
complete -c opencoding -n '__fish_use_subcommand' -a mcp -d 'Manage MCP servers'
complete -c opencoding -n '__fish_use_subcommand' -a skill -d 'Manage Skills'
complete -c opencoding -n '__fish_use_subcommand' -a hook -d 'Manage Hooks'
complete -c opencoding -n '__fish_use_subcommand' -a plugin -d 'Manage Plugins and Marketplaces'
complete -c opencoding -n '__fish_use_subcommand' -a app -d 'List Plugin Apps'
complete -c opencoding -n '__fish_use_subcommand' -a client -d 'List or revoke connected clients'
complete -c opencoding -n '__fish_use_subcommand' -a completion -d 'Generate shell completion'"#
        }
        "powershell" => {
            r#"Register-ArgumentCompleter -Native -CommandName opencoding -ScriptBlock {
  param($wordToComplete)
  'exec','review','doctor','sandbox','mcp','skill','hook','plugin','app','client','completion' | Where-Object { $_ -like "$wordToComplete*" }
}"#
        }
        _ => unreachable!("shell is validated by parse_args"),
    }
}
