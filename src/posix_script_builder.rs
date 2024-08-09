use std::{collections::VecDeque, fmt::Display, net::SocketAddr};

use bitcoin::{hex::DisplayHex, Block};
use serde::Serialize;

use crate::cli::RpcAuth;

#[derive(Debug)]
struct Command {
    command: String,
    args: Vec<String>,
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::iter::once(self.command.clone())
            .chain(self.args.clone())
            .collect::<Vec<_>>()
            .join(" ")
            .fmt(f)
    }
}

#[derive(Debug)]
struct Comment(String);

impl Display for Comment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0
            .lines()
            .map(|line| format!("# {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            .fmt(f)
    }
}

#[derive(Debug)]
enum ScriptItem {
    Command(Command),
    Comment(Comment),
}

#[derive(Debug)]
pub struct OutputPosixScriptBuilder {
    rpc_addr: SocketAddr,
    rpc_auth: RpcAuth,
    script: VecDeque<ScriptItem>,
}

impl OutputPosixScriptBuilder {
    pub fn new(rpc_addr: SocketAddr, rpc_auth: RpcAuth) -> Self {
        Self {
            rpc_addr,
            rpc_auth,
            script: VecDeque::new(),
        }
    }

    pub fn command<S>(&mut self, command: S, args: Vec<String>)
    where
        String: From<S>,
    {
        self.script.push_back(ScriptItem::Command(Command {
            command: command.into(),
            args,
        }))
    }

    pub fn comment<S>(&mut self, comment: S)
    where
        String: From<S>,
    {
        self.script
            .push_back(ScriptItem::Comment(Comment(comment.into())))
    }

    pub fn finalize(self) -> String {
        let mut res = "".to_owned();
        let mut iter = self.script.into_iter().peekable();
        while let Some(script_item) = iter.next() {
            match script_item {
                ScriptItem::Comment(comment) => {
                    res.push_str(&comment.to_string());
                    match iter.peek() {
                        Some(ScriptItem::Comment(_)) => {
                            res.push_str("\n\n");
                        }
                        Some(ScriptItem::Command(_)) | None => {
                            res.push('\n');
                        }
                    }
                }
                ScriptItem::Command(command) => {
                    res.push_str(&command.to_string());
                    if iter.peek().is_some() {
                        res.push_str("\n\n");
                    } else {
                        res.push('\n');
                    }
                }
            }
        }
        res
    }

    /// Use curl to send an RPC request to the node
    pub fn curl_rpc<Params>(&mut self, method: &str, params: Params)
    where
        Params: Serialize,
    {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "bip347-enforcer-test",
            "method": method,
            "params": params
        });
        let args = vec![
            format!("'{}'", &self.rpc_addr),
            "-H".to_owned(),
            "'Content-Type: application/json'".to_owned(),
            "--user".to_owned(),
            format!("'{}:{}'", self.rpc_auth.rpc_user, self.rpc_auth.rpc_pass),
            "--data-binary".to_owned(),
            format!("'{}'", serde_json::to_string(&request).unwrap()),
        ];
        let () = self.command("curl", args);
    }

    /// RPC request for `submitblock`
    pub fn submitblock(&mut self, block: &Block) {
        self.curl_rpc(
            "submitblock",
            [bitcoin::consensus::serialize(block).to_lower_hex_string()],
        )
    }
}
