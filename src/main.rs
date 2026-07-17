use base64::{Engine as _, engine::general_purpose};
use clap::{ArgAction, CommandFactory, Parser, Subcommand, ValueEnum};
use mcp_cli::{McpClient, McpConnection, ServerConfig, StdioClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod update_policy;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const ASK_BRIDGE_CHROME_MARKER: &str = "--ask-bridge-instance";
const CHROME_WINDOW_SIZE_ARG: &str = "--window-size=1440,1200";
const CHROME_VISIBLE_WINDOW_POSITION_ARG: &str = "--window-position=80,80";
const CHROME_BACKGROUND_WINDOW_POSITION_ARG: &str = "--window-position=-2000,-2000";
const CHROME_VISIBLE_WINDOW_X: i32 = 80;
const CHROME_VISIBLE_WINDOW_Y: i32 = 80;
const CHROME_VISIBLE_WINDOW_WIDTH: i32 = 1200;
const CHROME_VISIBLE_WINDOW_HEIGHT: i32 = 900;
const CHROME_MIN_VISIBLE_WIDTH: i32 = 320;
const CHROME_MIN_VISIBLE_HEIGHT: i32 = 240;
const COPILOT_ATTACHMENT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const COPILOT_ATTACHMENT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const COPILOT_ATTACHMENT_INDICATOR_SELECTOR: &str = concat!(
    "[data-testid*='attachment-chip' i],",
    "[data-testid*='file-chip' i],",
    "[data-testid*='attachment-card' i],",
    "[data-testid*='file-card' i],",
    "[data-testid*='attachment-preview' i],",
    "[data-testid*='image-preview' i],",
    "[class*='attachmentchip' i],",
    "[class*='filechip' i],",
    "[class*='attachmentcard' i],",
    "[class*='filecard' i],",
    "[class*='attachment-card' i],",
    "[class*='file-card' i],",
    "[class*='attachmentpreview' i],",
    "[class*='imagepreview' i],",
    "[class*='attachment-preview' i],",
    "[class*='image-preview' i],",
    "[aria-label*='remove attachment' i],",
    "[title*='remove attachment' i],",
    "[aria-label*='delete attachment' i],",
    "[title*='delete attachment' i],",
    "[aria-label*='remove file' i],",
    "[title*='remove file' i],",
    "[aria-label*='delete file' i],",
    "[title*='delete file' i],",
    "[aria-label*='remove image' i],",
    "[title*='remove image' i],",
    "[aria-label*='delete image' i],",
    "[title*='delete image' i],",
    "[aria-label*='移除附件'],",
    "[title*='移除附件'],",
    "[aria-label*='刪除附件'],",
    "[title*='刪除附件'],",
    "[aria-label*='删除附件'],",
    "[title*='删除附件'],",
    "[aria-label*='移除檔案'],",
    "[title*='移除檔案'],",
    "[aria-label*='刪除檔案'],",
    "[title*='刪除檔案'],",
    "[aria-label*='移除文件'],",
    "[title*='移除文件'],",
    "[aria-label*='删除文件'],",
    "[title*='删除文件']"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginState {
    LoggedIn,
    LoggedOut,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct LoginSignals {
    account: bool,
    auth_control: bool,
    auth_path: bool,
    composer: bool,
    stable: bool,
}

impl LoginSignals {
    fn state(self, provider: Provider) -> LoginState {
        if self.auth_path {
            LoginState::LoggedOut
        } else if self.account {
            LoginState::LoggedIn
        } else if !self.stable {
            LoginState::Unknown
        } else if self.auth_control {
            LoginState::LoggedOut
        } else if self.composer && matches!(provider, Provider::ChatGpt | Provider::Copilot) {
            LoginState::LoggedIn
        } else {
            LoginState::Unknown
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Provider {
    #[value(name = "chatgpt")]
    ChatGpt,
    #[value(name = "gemini")]
    Gemini,
    #[value(name = "claude")]
    Claude,
    #[value(name = "copilot")]
    Copilot,
}

impl Provider {
    fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatgpt" | "chat-gpt" | "chat_gpt" => Some(Provider::ChatGpt),
            "gemini" => Some(Provider::Gemini),
            "claude" | "claude-ai" | "claude_ai" | "claudeai" => Some(Provider::Claude),
            "copilot" | "m365-copilot" | "m365_copilot" => Some(Provider::Copilot),
            _ => None,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Provider::ChatGpt => "ChatGPT",
            Provider::Gemini => "Gemini",
            Provider::Claude => "Claude",
            Provider::Copilot => "Microsoft 365 Copilot",
        }
    }

    fn home_url(self) -> &'static str {
        match self {
            Provider::ChatGpt => "https://chatgpt.com/",
            Provider::Gemini => "https://gemini.google.com/app",
            Provider::Claude => "https://claude.ai/new",
            Provider::Copilot => "https://m365.cloud.microsoft/chat/",
        }
    }

    fn owns_url(self, url: &str) -> bool {
        match self {
            Provider::ChatGpt => url.contains("chatgpt.com"),
            Provider::Gemini => url.contains("gemini.google.com"),
            Provider::Claude => url.contains("claude.ai"),
            Provider::Copilot => url.contains("m365.cloud.microsoft"),
        }
    }

    fn from_url(url: &str) -> Option<Self> {
        [
            Provider::ChatGpt,
            Provider::Gemini,
            Provider::Claude,
            Provider::Copilot,
        ]
        .into_iter()
        .find(|provider| provider.owns_url(url))
    }

    fn ready_check_js(self) -> &'static str {
        match self {
            Provider::ChatGpt => r#"() => document.getElementById('prompt-textarea') !== null"#,
            Provider::Gemini => {
                r#"() => {
                    return document.querySelector('div[role="textbox"][aria-label*="Gemini"]') !== null ||
                           document.querySelector('rich-textarea [contenteditable="true"]') !== null ||
                           document.querySelector('.ql-editor[contenteditable="true"]') !== null ||
                           document.querySelector('a[href*="accounts.google.com"]') !== null ||
                           /Sign in|登入/.test(document.body.innerText || '');
                }"#
            }
            Provider::Claude => {
                r#"() => {
                    return document.querySelector('div[contenteditable="true"][data-testid="chat-input"]') !== null ||
                           document.querySelector('div[contenteditable="true"].ProseMirror') !== null ||
                           document.querySelector('[data-testid="login-with-google"]') !== null ||
                           window.location.pathname.startsWith('/login') ||
                           /Sign in|登入/.test(document.body.innerText || '');
                }"#
            }
            Provider::Copilot => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 && rect.height > 0;
                    };
                    const textFor = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.textContent
                    ].filter(Boolean).join(' ').trim();
                    const composer = [
                        'textarea#userInput',
                        'textarea[data-testid*="chat-input"]',
                        '[contenteditable="true"][role="textbox"]'
                    ].flatMap((selector) => Array.from(document.querySelectorAll(selector)))
                        .find(isVisible);
                    const authControl = Array.from(document.querySelectorAll('a, button, [role="button"]'))
                        .some((el) => isVisible(el) &&
                            /sign in|log in|login|登入|登錄|登录/i.test(textFor(el)));
                    return Boolean(composer || authControl);
                }"#
            }
        }
    }

    fn login_signals_js(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r#"async () => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };

                    const textFor = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.textContent
                    ].filter(Boolean).join(' ').trim();

                    const readSignals = () => {
                        const visibleAuthButton = Array.from(document.querySelectorAll('a, button'))
                            .some((el) => {
                                if (!isVisible(el)) return false;
                                const text = textFor(el);
                                return /^(log in|login|sign in|sign up|登入|登錄|登录|註冊|注册)$/i.test(text);
                            });

                        const composer = document.querySelector('#prompt-textarea') ||
                            document.querySelector('[data-testid="composer-text-input"]') ||
                            document.querySelector('textarea[placeholder*="Message"]') ||
                            document.querySelector('textarea[placeholder*="訊息"]') ||
                            document.querySelector('[contenteditable="true"]');

                        const accountMenu = document.querySelector('[data-testid="profile-button"]') ||
                            document.querySelector('[data-testid="account-menu-button"]') ||
                            document.querySelector('[data-testid="user-menu-button"]') ||
                            document.querySelector('button[aria-label*="Profile"]') ||
                            document.querySelector('button[aria-label*="profile"]') ||
                            document.querySelector('button[aria-label*="Account"]') ||
                            document.querySelector('button[aria-label*="account"]') ||
                            document.querySelector('button[aria-label*="User"]') ||
                            document.querySelector('button[aria-label*="user"]') ||
                            document.querySelector('button[aria-label*="帳戶"]') ||
                            document.querySelector('button[aria-label*="使用者"]');

                        return {
                            account: isVisible(accountMenu),
                            auth_control: Boolean(visibleAuthButton),
                            auth_path: /\/(auth|login|signup)(\/|$)/i.test(window.location.pathname),
                            composer: isVisible(composer)
                        };
                    };

                    let signals = readSignals();
                    let signature = JSON.stringify(signals);
                    const startedAt = Date.now();
                    let stableSince = startedAt;
                    let stable = false;
                    const earliestDecision = startedAt + 2000;
                    const deadline = Date.now() + 5000;
                    while (!signals.account && !signals.auth_path && Date.now() < deadline) {
                        await new Promise((resolve) => setTimeout(resolve, 250));
                        const nextSignals = readSignals();
                        const nextSignature = JSON.stringify(nextSignals);
                        if (nextSignature !== signature) {
                            signature = nextSignature;
                            stableSince = Date.now();
                        }
                        signals = nextSignals;
                        if (Date.now() >= earliestDecision && Date.now() - stableSince >= 750) {
                            stable = true;
                            break;
                        }
                    }

                    return { ...signals, stable };
                }"#
            }
            Provider::Gemini => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };
                    const composer = document.querySelector('div[role="textbox"][aria-label*="Gemini"]') ||
                        document.querySelector('rich-textarea [contenteditable="true"]') ||
                        document.querySelector('.ql-editor[contenteditable="true"]');
                    const account = document.querySelector('a[href*="accounts.google.com/SignOutOptions"]') ||
                        document.querySelector('[aria-label*="Google 帳戶"]') ||
                        document.querySelector('[aria-label*="Google Account"]');
                    const signIn = Array.from(document.querySelectorAll('a, button'))
                        .some((el) => isVisible(el) && /Sign in|登入/.test([
                                el.getAttribute('aria-label'),
                                el.textContent
                            ].filter(Boolean).join(' ')));
                    const authPath = /\/(auth|login|signin|signup)(\/|$)/i.test(window.location.pathname);
                    return {
                        account: isVisible(account),
                        auth_control: Boolean(signIn),
                        auth_path: authPath,
                        composer: Boolean(composer),
                        stable: true
                    };
                }"#
            }
            Provider::Claude => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 &&
                            rect.height > 0;
                    };
                    const composer = document.querySelector('div[contenteditable="true"][data-testid="chat-input"]') ||
                        document.querySelector('div[contenteditable="true"].ProseMirror');
                    const account = document.querySelector('[data-testid="user-menu-button"]') ||
                        document.querySelector('button[aria-label*="User menu"]') ||
                        document.querySelector('button[aria-label*="Account"]');
                    const signIn = document.querySelector('[data-testid="login-with-google"]') ||
                        Array.from(document.querySelectorAll('a, button'))
                            .find((el) => isVisible(el) && /^(log in|login|sign in|sign up|登入|註冊)$/i.test([
                                    el.getAttribute('aria-label'),
                                    el.textContent
                                ].filter(Boolean).join(' ').trim()));
                    const authPath = /^\/(login|signup|magic-link)(\/|$)/i.test(window.location.pathname);
                    return {
                        account: isVisible(account),
                        auth_control: Boolean(signIn),
                        auth_path: authPath,
                        composer: Boolean(composer)
                    };
                }"#
            }
            Provider::Copilot => {
                r#"() => {
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        const rect = el.getBoundingClientRect();
                        return style.display !== 'none' &&
                            style.visibility !== 'hidden' &&
                            style.opacity !== '0' &&
                            rect.width > 0 && rect.height > 0;
                    };
                    const textFor = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.textContent
                    ].filter(Boolean).join(' ').trim();
                    const composer = [
                        'textarea#userInput',
                        'textarea[data-testid*="chat-input"]',
                        '[contenteditable="true"][role="textbox"]'
                    ].flatMap((selector) => Array.from(document.querySelectorAll(selector)))
                        .find(isVisible);
                    const controls = Array.from(document.querySelectorAll('a, button, [role="button"]'));
                    const account = controls.find((el) => {
                        if (!isVisible(el)) return false;
                        const testId = el.getAttribute('data-testid') || '';
                        const signal = `${testId} ${textFor(el)}`;
                        return /account[-_ ]?(manager|menu)|profile[-_ ]?(menu|button)|sign out|log out|logout|帳戶管理|帐户管理|個人檔案|个人资料|登出|退出登錄|退出登录|註銷|注销/i.test(signal);
                    });
                    const signIn = controls.find((el) => isVisible(el) &&
                        /sign in|log in|login|登入|登錄|登录/i.test(textFor(el)));
                    return {
                        account: isVisible(account),
                        auth_control: Boolean(signIn),
                        auth_path: /\/(login|signin|oauth|auth)(\/|$)/i.test(window.location.pathname),
                        composer: isVisible(composer),
                        stable: true
                    };
                }"#
            }
        }
    }

    fn assistant_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => "[data-message-author-role=\"assistant\"], .agent-turn",
            Provider::Gemini => "model-response",
            Provider::Claude => ".font-claude-response",
            Provider::Copilot => {
                "[data-content=\"ai-message\"], [data-testid*=\"assistant\"], [data-testid*=\"response\"], [data-author=\"assistant\"], [class*=\"AIMessage\"], [class*=\"AiMessage\"], .fai-CopilotMessage"
            }
        }
    }

    fn latest_response_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                "[data-message-author-role=\"assistant\"], .agent-turn, model-response, .model-response, [data-test-id*=\"response\"], [data-testid*=\"response\"]"
            }
            Provider::Gemini => "model-response",
            Provider::Claude => ".font-claude-response",
            Provider::Copilot => {
                "[data-content=\"ai-message\"], [data-testid*=\"assistant\"], [data-testid*=\"response\"], [data-author=\"assistant\"], [class*=\"AIMessage\"], [class*=\"AiMessage\"], .fai-CopilotMessage"
            }
        }
    }

    fn response_content_selector(self) -> &'static str {
        match self {
            Provider::ChatGpt => "",
            Provider::Gemini => {
                "message-content, .markdown, structured-content-container.model-response-text"
            }
            Provider::Claude => ".standard-markdown, .font-claude-response-body",
            Provider::Copilot => {
                ".fai-CopilotMessage__content, [data-testid*=\"message-content\"], .markdown, .ac-textBlock, [class*=\"markdown\"], [class*=\"MessageContent\"], [class*=\"ResponseContent\"], [class*=\"ResponseRenderer\"]"
            }
        }
    }

    fn composer_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => r##"["#prompt-textarea"]"##,
            Provider::Gemini => {
                r#"[
                    "div[role=\"textbox\"][aria-label*=\"Gemini\"]",
                    "rich-textarea [contenteditable=\"true\"]",
                    ".ql-editor[contenteditable=\"true\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "div[contenteditable=\"true\"][data-testid=\"chat-input\"]",
                    "div[contenteditable=\"true\"].ProseMirror",
                    "div[aria-label*=\"Claude\"][contenteditable=\"true\"]"
                ]"#
            }
            Provider::Copilot => {
                r#"[
                    "textarea#userInput",
                    "textarea[data-testid*=\"chat-input\"]",
                    "[contenteditable=\"true\"][role=\"textbox\"]"
                ]"#
            }
        }
    }

    fn send_button_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r##"[
                    "[data-testid=\"send-button\"]",
                    "#composer-submit-button",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]",
                    "button[aria-label*=\"发送\"]"
                ]"##
            }
            Provider::Gemini => {
                r#"[
                    "button[aria-label=\"傳送訊息\"]",
                    "button[aria-label=\"Submit\"]",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]",
                    "button[aria-label*=\"提交\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "button[aria-label=\"Send message\"]",
                    "button[aria-label*=\"Send\"]",
                    "button[aria-label*=\"傳送\"]"
                ]"#
            }
            Provider::Copilot => {
                r#"[
                    "button[class*=\"SendButton\"][type=\"submit\"]",
                    "button[type=\"submit\"][aria-label]",
                    "button[data-testid*=\"submit\"]",
                    "button[data-testid*=\"send\"]",
                    "button[aria-label=\"Send\"]",
                    "button[aria-label=\"Submit\"]",
                    "button[aria-label*=\"Send message\"]",
                    "button[aria-label*=\"傳送\"]",
                    "button[aria-label*=\"发送\"]"
                ]"#
            }
        }
    }

    fn stop_button_selectors_json(self) -> &'static str {
        match self {
            Provider::ChatGpt => {
                r##"[
                    "[data-testid=\"stop-button\"]",
                    "#composer-stop-button",
                    "button[aria-label=\"Stop generating\"]"
                ]"##
            }
            Provider::Gemini => {
                r#"[
                    "button[aria-label=\"停止回覆\"]",
                    "button[aria-label*=\"Stop\"]",
                    "button[aria-label*=\"停止\"]"
                ]"#
            }
            Provider::Claude => {
                r#"[
                    "button[aria-label=\"Stop response\"]",
                    "button[aria-label*=\"Stop\"]",
                    "button[aria-label*=\"停止\"]"
                ]"#
            }
            Provider::Copilot => {
                r#"[
                    "button[data-testid*=\"stop\"]",
                    "button[class*=\"SendButton\"][aria-label*=\"Stop\"]",
                    "button[class*=\"SendButton\"][aria-label*=\"停止\"]",
                    "button[type=\"submit\"][aria-label*=\"Stop\"]",
                    "button[type=\"submit\"][aria-label*=\"停止\"]",
                    "button[aria-label*=\"Stop generating\"]",
                    "button[aria-label*=\"Stop responding\"]",
                    "button[aria-label=\"Stop\"]",
                    "button[aria-label*=\"停止產生\"]",
                    "button[aria-label*=\"停止回覆\"]",
                    "button[aria-label*=\"停止回應\"]"
                ]"#
            }
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::ChatGpt => write!(f, "chatgpt"),
            Provider::Gemini => write!(f, "gemini"),
            Provider::Claude => write!(f, "claude"),
            Provider::Copilot => write!(f, "copilot"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ChatGptAgentPrompt<'a> {
    agent_mention: &'a str,
    body: &'a str,
}

fn parse_chatgpt_agent_prompt(prompt: &str) -> Option<ChatGptAgentPrompt<'_>> {
    let rest = prompt.strip_prefix('@')?;
    let mut agent_chars = 0usize;

    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            if agent_chars == 0 || agent_chars > 10 {
                return None;
            }

            let body = rest[idx + ch.len_utf8()..].trim_start_matches(char::is_whitespace);
            if body.is_empty() {
                return None;
            }

            return Some(ChatGptAgentPrompt {
                agent_mention: &prompt[..idx + 1],
                body,
            });
        }

        agent_chars += 1;
        if agent_chars > 10 {
            return None;
        }
    }

    None
}

#[derive(Parser)]
#[command(name = "ask-bridge")]
#[command(version = "0.3.5")]
#[command(disable_version_flag = true)]
#[command(about = "AI browser CLI - Ask ChatGPT, Gemini, Claude or Microsoft 365 Copilot from your Terminal with your subscription", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// The prompt to send to the selected provider.
    /// If standard input is piped and this value is present, they are combined as:
    /// `prompt + "\\n\\n" + stdin`.
    prompt: Option<String>,

    /// AI provider to automate. Overrides ~/.config/ask-bridge/config.json.
    #[arg(long, short = 'p', value_enum, global = true)]
    provider: Option<Provider>,

    /// Run Chrome in headless mode. Defaults to true.
    #[arg(long, require_equals = true, num_args = 0..=1, default_value = "true", default_missing_value = "true")]
    headless: bool,

    /// Create a brand new provider session by opening a new tab and closing old ones.
    #[arg(long, default_value_t = false)]
    new: bool,

    /// Print version information.
    #[arg(
        long = "version",
        short = 'v',
        short_alias = 'V',
        action = ArgAction::Version
    )]
    _version: (),

    /// Print verbose debugging status messages.
    #[arg(long, default_value_t = false)]
    verbose: bool,

    /// Write the final response in Markdown format to the specified file.
    #[arg(long, short, value_name = "FILE")]
    output: Option<String>,

    /// Write the downloaded images to the specified folder or file path.
    #[arg(long, short = 'i', value_name = "IMAGE_PATH")]
    image_output: Option<String>,

    /// Attach one or more local image files to the prompt (can be specified multiple times).
    #[arg(long = "image", value_name = "IMAGE_FILE", num_args = 1)]
    images: Vec<String>,

    /// Attach one or more local document files (PDF, Word, Excel, text, etc.) to the prompt
    /// (can be specified multiple times).
    #[arg(long = "file", value_name = "FILE", num_args = 1)]
    files: Vec<String>,

    /// Maximum time in seconds to wait for the provider response.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,

    /// Switch the provider model before sending the prompt.
    /// ChatGPT examples: "GPT-5.5", "GPT-5.4", "GPT-5.3", "o3", or thinking levels such as
    /// "即時", "中等", "高", "超高", "專業", "智慧". Gemini examples: "3.5 Flash",
    /// "3.1 Flash-Lite", or "3.1 Pro". Claude examples: "Sonnet", "Opus", "Haiku".
    /// Matching is case- and punctuation-insensitive.
    #[arg(long = "model", value_name = "MODEL")]
    model: Option<String>,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Open Chrome browser, optionally navigate to a URL, and copy the latest response
    #[command(hide = true)]
    Open {
        /// Optional conversation URL to open before copying the latest response.
        url: Option<String>,
    },
    /// Retrieve the latest response from the selected provider (defaults to headless)
    #[command(hide = true)]
    Get {
        /// Optional conversation URL to fetch before copying the latest response.
        url: Option<String>,
        /// Print verbose debugging status messages.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
    /// Open Chrome browser and wait for manual login
    Login,
    /// Close the managed Chrome browser instance
    Close,
    /// Set or show the global default provider used when --provider is not specified.
    Config,
    /// Reinstall ask-bridge using the recommended README installation command
    Update,
    /// Dump the current browser tab HTML for debugging
    #[command(hide = true)]
    Dump,
    /// Take a screenshot of the current browser tab for debugging
    #[command(hide = true)]
    Screenshot,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AppConfig {
    provider: Option<String>,
}

fn config_file_path() -> Result<PathBuf, String> {
    let mut config_path = home::home_dir().ok_or("Could not locate home directory")?;
    config_path.push(".config/ask-bridge/config.json");
    Ok(config_path)
}

fn parse_configured_provider(content: &str) -> Result<Option<Provider>, String> {
    let config: AppConfig =
        serde_json::from_str(content).map_err(|e| format!("Failed to parse config.json: {}", e))?;

    match config.provider {
        Some(provider) => Provider::from_config_value(&provider)
            .map(Some)
            .ok_or_else(|| format!("Invalid provider in config.json: {}", provider)),
        None => Ok(None),
    }
}

fn load_configured_provider() -> Result<Option<Provider>, String> {
    let config_path = config_file_path()?;
    if !config_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path).map_err(|e| {
        format!(
            "Failed to read config file {}: {}",
            config_path.to_string_lossy(),
            e
        )
    })?;

    parse_configured_provider(&content).map_err(|e| {
        format!(
            "{}. Expected provider: chatgpt, gemini, claude, or copilot",
            e
        )
    })
}

fn effective_provider(
    cli_provider: Option<Provider>,
    configured_provider: Option<Provider>,
) -> Provider {
    cli_provider
        .or(configured_provider)
        .unwrap_or(Provider::ChatGpt)
}

fn resolve_provider_with<F>(
    cli_provider: Option<Provider>,
    load_provider: F,
) -> Result<Provider, String>
where
    F: FnOnce() -> Result<Option<Provider>, String>,
{
    if let Some(provider) = cli_provider {
        return Ok(provider);
    }

    Ok(effective_provider(None, load_provider()?))
}

fn resolve_provider(cli_provider: Option<Provider>) -> Result<Provider, String> {
    resolve_provider_with(cli_provider, load_configured_provider)
}

fn write_global_provider_config(provider: Provider) -> Result<(), String> {
    let config_path = config_file_path()?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory {}: {}",
                parent.to_string_lossy(),
                e
            )
        })?;
    }

    let content =
        serde_json::to_string_pretty(&serde_json::json!({"provider": provider.to_string()}))
            .map_err(|e| format!("Failed to serialize provider config: {}", e))?;
    std::fs::write(&config_path, format!("{}\n", content)).map_err(|e| {
        format!(
            "Failed to write config file {}: {}",
            config_path.to_string_lossy(),
            e
        )
    })?;

    println!(
        "Set default provider to '{}' in {}",
        provider,
        config_path.to_string_lossy()
    );

    Ok(())
}

fn run_config_command(cli_provider: Option<Provider>) -> Result<(), String> {
    match cli_provider {
        Some(provider) => write_global_provider_config(provider),
        None => {
            let config_path = config_file_path()?;
            let configured_provider = load_configured_provider()?;
            match configured_provider {
                Some(provider) => {
                    println!("Current default provider: {}", provider);
                }
                None => {
                    println!("No default provider configured.");
                    println!("The effective provider is ChatGPT.");
                }
            }
            if config_path.exists() {
                println!("Config file: {}", config_path.to_string_lossy());
            } else {
                println!(
                    "Config file not created yet: {}",
                    config_path.to_string_lossy()
                );
            }
            println!(
                "Set default provider with: ask-bridge config --provider <chatgpt|gemini|claude|copilot>"
            );
            println!("This is a one-time override example: ask-bridge --provider gemini <prompt>");
            Ok(())
        }
    }
}

fn run_update_command() -> Result<(), String> {
    println!("Running ask-bridge update via official installer...");
    println!("Progress: downloading installer and updating binary.");

    #[cfg(target_os = "windows")]
    {
        // Never execute an adjacent helper of unknown version or provenance.
        // The detached PowerShell process holds an identity-bound Process
        // handle, waits for this executable to exit, then downloads and
        // verifies the signed installer using the compiled update policy.
        let update_command = update_policy::windows_update_command_after_parent(
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
        );
        let child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                update_command.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to launch signed Windows updater: {}", e))?;
        println!("Progress: signed updater started with PID {}.", child.id());
        println!("Progress: update command is running in background.");
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let update_command = update_policy::unix_update_command(env!("CARGO_PKG_VERSION"));
        let status = Command::new("bash")
            .args(["-c", update_command.as_str()])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("Failed to run macOS/Linux update command: {}", e))?;

        if status.success() {
            println!("Progress: update command completed.");
            Ok(())
        } else {
            Err(format!("Update command failed with exit status {}", status))
        }
    }
}

struct Page {
    id: usize,
    url: String,
    selected: bool,
}

#[derive(Clone, Copy, Debug)]
struct PageLoginState {
    id: usize,
    selected: bool,
    login_state: LoginState,
}

fn preferred_provider_page_id(pages: &[PageLoginState]) -> Option<usize> {
    pages
        .iter()
        .find(|page| page.login_state == LoginState::LoggedIn)
        .or_else(|| pages.iter().find(|page| page.selected))
        .or_else(|| pages.first())
        .map(|page| page.id)
}

fn parse_node_version(output: &str) -> Option<(u64, u64, u64)> {
    let version = output.trim().strip_prefix('v').unwrap_or(output.trim());
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;

    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

fn validate_node_version_output(output: &str) -> Result<(), String> {
    let version = parse_node_version(output).ok_or_else(|| {
        format!(
            "Could not parse Node.js version from '{}'. Install a current Node.js LTS release and retry.",
            output.trim()
        )
    })?;
    let (major, minor, patch) = version;
    let supported = (major == 20 && (minor, patch) >= (19, 0))
        || (major == 22 && (minor, patch) >= (12, 0))
        || major >= 23;

    if supported {
        return Ok(());
    }

    Err(format!(
        "Node.js v{major}.{minor}.{patch} is not supported by {MCP_PACKAGE_SPEC}. Supported versions are ^20.19.0, ^22.12.0, or >=23.0.0. Install a current Node.js LTS release, reopen the terminal, and retry."
    ))
}

fn check_node_runtime() -> Result<(), String> {
    let output = Command::new("node")
        .arg("--version")
        .output()
        .map_err(|e| {
            format!(
                "Failed to run 'node --version': {e}. Install Node.js and ensure it is available in PATH."
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "'node --version' exited with status {}. Install a current Node.js LTS release and retry.",
            output.status
        ));
    }

    validate_node_version_output(&String::from_utf8_lossy(&output.stdout))
}

/// Pinned chrome-devtools-mcp package spec. `@latest` would make every npx
/// spawn re-resolve the dist-tag against the npm registry, which was observed
/// stalling; with mcp-cli's timeout-less request wait that hung whole runs
/// (2026-07-11). Bump this version deliberately and re-run the e2e check.
const MCP_PACKAGE_SPEC: &str = "chrome-devtools-mcp@1.5.0";

fn build_chrome_devtools_server_config(
    quiet_mcp: bool,
    headless: bool,
    log_path: &str,
    is_windows: bool,
) -> Value {
    let mut mcp_args = vec![
        "-y".to_string(),
        MCP_PACKAGE_SPEC.to_string(),
        "--browser-url=http://127.0.0.1:9223".to_string(),
    ];
    if quiet_mcp {
        mcp_args.push("--no-usage-statistics".to_string());
        mcp_args.push("--no-performance-crux".to_string());
    }
    if headless {
        mcp_args.push("--headless".to_string());
    }
    mcp_args.push("--logFile".to_string());
    mcp_args.push(log_path.to_string());

    let mut chrome_devtools_server = serde_json::json!({
        "command": if is_windows { "npx.cmd" } else { "npx" },
        "args": mcp_args
    });

    if quiet_mcp {
        chrome_devtools_server["env"] = serde_json::json!({
            "NPM_CONFIG_LOGLEVEL": "error",
            "NPM_CONFIG_PROGRESS": "false",
            "NPM_CONFIG_FUND": "false",
            "NPM_CONFIG_AUDIT": "false",
            "NPM_CONFIG_FUNDING": "0",
            "NPM_CONFIG_UPDATE_NOTIFIER": "false",
            "NO_COLOR": "1",
            "CI": "1",
            "NODE_NO_WARNINGS": "1"
        });
    }

    chrome_devtools_server
}

fn write_mcp_config(quiet_mcp: bool, headless: bool) -> Result<String, String> {
    let mut config_dir = home::home_dir().ok_or("Could not locate home directory")?;
    config_dir.push(".config/ask-bridge");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let log_path = config_dir
        .join("chrome-devtools-mcp.log")
        .to_string_lossy()
        .to_string();

    config_dir.push("mcp_servers.json");
    let config_path = config_dir.to_string_lossy().to_string();

    let chrome_devtools_server = build_chrome_devtools_server_config(
        quiet_mcp,
        headless,
        &log_path,
        cfg!(target_os = "windows"),
    );

    let config_content = serde_json::json!({
        "mcpServers": {
            "chrome-devtools": chrome_devtools_server
        }
    });

    let content_str = serde_json::to_string_pretty(&config_content).map_err(|e| e.to_string())?;

    std::fs::write(&config_path, content_str)
        .map_err(|e| format!("Failed to write mcp_servers.json: {}", e))?;

    Ok(config_path)
}

fn chrome_profile_path() -> Result<String, String> {
    let mut profile_dir = home::home_dir().ok_or("Could not locate home directory")?;
    profile_dir.push(".config/ask-bridge/chrome-profile");
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("Failed to create chrome profile directory: {}", e))?;

    Ok(profile_dir.to_string_lossy().to_string())
}

fn chrome_pid_path() -> Result<PathBuf, String> {
    let mut path = home::home_dir().ok_or("Could not locate home directory")?;
    path.push(".config/ask-bridge/chrome.pid");
    Ok(path)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct ChromeProcessRecord {
    pid: u32,
    #[serde(default)]
    browser_id: Option<String>,
}

fn parse_chrome_process_record(content: &str) -> Option<ChromeProcessRecord> {
    serde_json::from_str(content).ok().or_else(|| {
        content
            .trim()
            .parse::<u32>()
            .ok()
            .map(|pid| ChromeProcessRecord {
                pid,
                browser_id: None,
            })
    })
}

fn write_chrome_process_record(record: &ChromeProcessRecord) -> Result<(), String> {
    let path = chrome_pid_path()?;
    let content = serde_json::to_string(record)
        .map_err(|e| format!("Failed to serialize Chrome process record: {}", e))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

fn read_chrome_process_record() -> Option<ChromeProcessRecord> {
    let path = chrome_pid_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_chrome_process_record(&content)
}

fn read_chrome_pid() -> Option<String> {
    read_chrome_process_record().map(|record| record.pid.to_string())
}

fn remove_chrome_pid_file() -> Result<(), String> {
    let path = chrome_pid_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove {}: {}", path.display(), e)),
    }
}

fn browser_id_from_websocket_url(url: &str) -> Option<String> {
    const LOOPBACK_PREFIXES: &[&str] = &[
        "ws://127.0.0.1:9223/devtools/browser/",
        "ws://localhost:9223/devtools/browser/",
        "ws://[::1]:9223/devtools/browser/",
    ];
    let id = LOOPBACK_PREFIXES
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))?
        .trim();
    (!id.is_empty() && !id.contains(['/', '?', '#'])).then(|| id.to_string())
}

fn browser_id_from_version_response(response: &str) -> Option<String> {
    if !http_response_is_complete(response.as_bytes()) {
        return None;
    }
    let (headers, body) = response.split_once("\r\n\r\n")?;
    let status = headers.lines().next()?;
    let mut status_parts = status.split_whitespace();
    if !status_parts.next()?.starts_with("HTTP/") || status_parts.next()? != "200" {
        return None;
    }
    let body = body.trim();
    let version: Value = serde_json::from_str(body).ok()?;
    let websocket_url = version.get("webSocketDebuggerUrl")?.as_str()?;
    browser_id_from_websocket_url(websocket_url)
}

fn http_response_is_complete(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let body_start = header_end + 4;
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let content_length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });

    content_length
        .and_then(|content_length| body_start.checked_add(content_length))
        .map(|response_length| response.len() >= response_length)
        .unwrap_or(false)
}

fn debug_browser_id() -> Option<String> {
    const MAX_RESPONSE_SIZE: usize = 64 * 1024;
    const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

    let mut stream = TcpStream::connect("127.0.0.1:9223").ok()?;
    let timeout = Some(Duration::from_millis(500));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    stream
        .write_all(
            b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9223\r\nConnection: close\r\n\r\n",
        )
        .ok()?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                response
                    .len()
                    .checked_add(bytes_read)
                    .filter(|length| *length <= MAX_RESPONSE_SIZE)
                    .map(|_| ())?;
                response.extend_from_slice(&buffer[..bytes_read]);
                if http_response_is_complete(&response) {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => return None,
        }
    }

    if !http_response_is_complete(&response) {
        return None;
    }
    let response = String::from_utf8(response).ok()?;
    browser_id_from_version_response(&response)
}

fn build_chrome_process_record(
    listener_pids: &[String],
    browser_id: Option<&str>,
) -> Option<ChromeProcessRecord> {
    if listener_pids.len() != 1 {
        return None;
    }
    Some(ChromeProcessRecord {
        pid: listener_pids.first()?.parse::<u32>().ok()?,
        browser_id: Some(browser_id?.to_string()),
    })
}

#[cfg(any(target_os = "linux", test))]
const LINUX_CHROME_COMMANDS: &[&str] = &["google-chrome", "google-chrome-stable"];

#[cfg(any(target_os = "linux", test))]
fn first_existing_path(paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
}

#[cfg(any(target_os = "linux", test))]
fn find_command_in_path(command: &str, path_env: Option<&std::ffi::OsStr>) -> Option<String> {
    let path_env = path_env?;

    std::env::split_paths(path_env)
        .map(|dir| dir.join(command))
        .find(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
}

#[cfg(any(target_os = "linux", test))]
fn find_chrome_command_in_path(path_env: Option<&std::ffi::OsStr>) -> Option<String> {
    LINUX_CHROME_COMMANDS
        .iter()
        .find_map(|command| find_command_in_path(command, path_env))
}

#[cfg(any(target_os = "linux", test))]
fn find_linux_chrome_path(
    path_env: Option<&std::ffi::OsStr>,
    path_candidates: &[&str],
) -> Option<String> {
    find_chrome_command_in_path(path_env).or_else(|| first_existing_path(path_candidates))
}

fn find_chrome_path() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        // 1. Program Files
        if let Ok(pf) = std::env::var("ProgramFiles") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", pf);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        } else {
            let path = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
            if std::path::Path::new(path).exists() {
                return Ok(path.to_string());
            }
        }

        // 2. Program Files (x86)
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", pf86);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        } else {
            let path = r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe";
            if std::path::Path::new(path).exists() {
                return Ok(path.to_string());
            }
        }

        // 3. LocalAppData
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let path = format!(r"{}\Google\Chrome\Application\chrome.exe", local_app_data);
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        }

        Err("Google Chrome was not found in standard Windows installation paths. Please install Google Chrome.".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        if std::path::Path::new(path).exists() {
            Ok(path.to_string())
        } else {
            Err("Google Chrome not found at /Applications/Google Chrome.app".to_string())
        }
    }

    #[cfg(target_os = "linux")]
    {
        const LINUX_CHROME_PATHS: &[&str] = &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/local/bin/google-chrome",
            "/usr/local/bin/google-chrome-stable",
            "/opt/google/chrome/google-chrome",
        ];

        let path_env = std::env::var_os("PATH");
        find_linux_chrome_path(path_env.as_deref(), LINUX_CHROME_PATHS).ok_or_else(|| {
            "Google Chrome was not found in PATH or standard Linux installation paths. Please install Google Chrome or add google-chrome to PATH.".to_string()
        })
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err("Google Chrome auto-detection is not supported on this operating system. Please use macOS, Windows, or Linux.".to_string())
    }
}

fn chrome_window_launch_args(headless: bool) -> &'static [&'static str] {
    if headless {
        &[
            "--ask-bridge-background",
            "--disable-blink-features=AutomationControlled",
            CHROME_WINDOW_SIZE_ARG,
            CHROME_BACKGROUND_WINDOW_POSITION_ARG,
        ]
    } else {
        &[CHROME_WINDOW_SIZE_ARG, CHROME_VISIBLE_WINDOW_POSITION_ARG]
    }
}

fn chrome_window_candidate_pids(snapshot: &ChromeDebugSnapshot) -> Vec<u32> {
    let mut pids = Vec::new();

    // The raw netstat/lsof listener list is not an identity proof. Only trust the
    // recorded PID when both its CDP browser id and its sole listener PID match,
    // plus owner PIDs which were independently verified from Chrome's command line.
    if chrome_record_matches_current(
        snapshot.record.as_ref(),
        snapshot.browser_id.as_deref(),
        &snapshot.listener_pids,
    ) && let Some(record) = snapshot.record.as_ref()
    {
        pids.push(record.pid);
    }

    for candidate in &snapshot.ask_pids {
        if let Ok(pid) = candidate.parse::<u32>()
            && !pids.contains(&pid)
        {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(target_os = "windows")]
fn chrome_image_paths_match(actual: &str, expected: &str) -> bool {
    const CSTR_EQUAL: i32 = 2;
    if actual.is_empty() || expected.is_empty() {
        return false;
    }

    let actual: Vec<u16> = actual.encode_utf16().collect();
    let expected: Vec<u16> = expected.encode_utf16().collect();
    let (Ok(actual_length), Ok(expected_length)) =
        (i32::try_from(actual.len()), i32::try_from(expected.len()))
    else {
        return false;
    };
    (unsafe {
        CompareStringOrdinal(
            actual.as_ptr(),
            actual_length,
            expected.as_ptr(),
            expected_length,
            1,
        )
    }) == CSTR_EQUAL
}

#[cfg(all(not(target_os = "windows"), test))]
fn chrome_image_paths_match(actual: &str, expected: &str) -> bool {
    !actual.is_empty() && !expected.is_empty() && actual.to_lowercase() == expected.to_lowercase()
}

#[cfg(any(target_os = "windows", test))]
fn chrome_window_matches_predicate(
    pid_is_validated: bool,
    class_name: &str,
    has_owner: bool,
    title_length: usize,
) -> bool {
    pid_is_validated && class_name == "Chrome_WidgetWin_1" && !has_owner && title_length > 0
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(test)]
fn screen_rects_intersect(left: ScreenRect, right: ScreenRect) -> bool {
    left.left < left.right
        && left.top < left.bottom
        && right.left < right.right
        && right.top < right.bottom
        && left.left < right.right
        && left.right > right.left
        && left.top < right.bottom
        && left.bottom > right.top
}

#[cfg(any(target_os = "windows", test))]
fn screen_rect_visible_area_at_least(
    window: ScreenRect,
    monitor: ScreenRect,
    minimum_width: i32,
    minimum_height: i32,
) -> bool {
    let visible_width = window.right.min(monitor.right) - window.left.max(monitor.left);
    let visible_height = window.bottom.min(monitor.bottom) - window.top.max(monitor.top);
    visible_width >= minimum_width && visible_height >= minimum_height
}

fn apply_chrome_window_mode_with<F>(
    headless: bool,
    snapshot: &ChromeDebugSnapshot,
    mut make_visible: F,
) -> Result<(), String>
where
    F: FnMut(&[u32]) -> Result<(), String>,
{
    if headless {
        return Ok(());
    }

    let pids = chrome_window_candidate_pids(snapshot);
    if pids.is_empty() {
        return Err(
            "Chrome is listening on port 9223, but its window process could not be identified"
                .to_string(),
        );
    }
    make_visible(&pids)
}

#[cfg(target_os = "windows")]
type NativeWindowHandle = *mut std::ffi::c_void;

#[cfg(target_os = "windows")]
type NativeProcessHandle = *mut std::ffi::c_void;

#[cfg(target_os = "windows")]
type NativeMonitorHandle = *mut std::ffi::c_void;

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
impl From<NativeRect> for ScreenRect {
    fn from(rect: NativeRect) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct NativeMonitorInfo {
    size: u32,
    monitor: NativeRect,
    work: NativeRect,
    flags: u32,
}

#[cfg(target_os = "windows")]
struct ValidatedChromeProcess {
    pid: u32,
    handle: NativeProcessHandle,
}

#[cfg(target_os = "windows")]
impl Drop for ValidatedChromeProcess {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct VisibleWindowContext<'a> {
    processes: &'a [ValidatedChromeProcess],
    moved: bool,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(
        first: *const u16,
        first_length: i32,
        second: *const u16,
        second_length: i32,
        ignore_case: i32,
    ) -> i32;
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> NativeProcessHandle;
    fn QueryFullProcessImageNameW(
        process: NativeProcessHandle,
        flags: u32,
        executable_name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(object: NativeProcessHandle) -> i32;
    fn WaitForSingleObject(object: NativeProcessHandle, timeout_ms: u32) -> u32;
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(
        callback: Option<unsafe extern "system" fn(NativeWindowHandle, isize) -> i32>,
        parameter: isize,
    ) -> i32;
    fn GetWindowThreadProcessId(window: NativeWindowHandle, pid: *mut u32) -> u32;
    fn GetClassNameW(window: NativeWindowHandle, class_name: *mut u16, max_count: i32) -> i32;
    fn GetWindow(window: NativeWindowHandle, command: u32) -> NativeWindowHandle;
    fn SendMessageTimeoutW(
        window: NativeWindowHandle,
        message: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        timeout_ms: u32,
        result: *mut usize,
    ) -> isize;
    fn SetWindowPos(
        window: NativeWindowHandle,
        insert_after: NativeWindowHandle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn SetForegroundWindow(window: NativeWindowHandle) -> i32;
    fn ShowWindowAsync(window: NativeWindowHandle, command: i32) -> i32;
    fn IsWindowVisible(window: NativeWindowHandle) -> i32;
    fn IsIconic(window: NativeWindowHandle) -> i32;
    fn GetWindowRect(window: NativeWindowHandle, rect: *mut NativeRect) -> i32;
    fn MonitorFromRect(rect: *const NativeRect, flags: u32) -> NativeMonitorHandle;
    fn GetMonitorInfoW(monitor: NativeMonitorHandle, info: *mut NativeMonitorInfo) -> i32;
}

#[cfg(target_os = "windows")]
fn query_full_process_image_name(process: NativeProcessHandle) -> Option<String> {
    // Windows' maximum extended path is 32,767 UTF-16 code units. Keep the
    // process handle alive after this query so its PID cannot be recycled while
    // EnumWindows is matching the corresponding top-level window.
    let mut path = vec![0_u16; 32_768];
    let mut length = u32::try_from(path.len()).ok()?;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    (length > 0 && length <= path.len())
        .then(|| String::from_utf16(&path[..length]).ok())
        .flatten()
}

#[cfg(target_os = "windows")]
fn open_validated_chrome_processes(
    pids: &[u32],
    expected_chrome_path: &str,
) -> Vec<ValidatedChromeProcess> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;

    let mut processes = Vec::new();
    for &pid in pids {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            continue;
        }

        let process = ValidatedChromeProcess { pid, handle };
        if query_full_process_image_name(process.handle)
            .as_deref()
            .is_some_and(|path| chrome_image_paths_match(path, expected_chrome_path))
        {
            processes.push(process);
        }
    }
    processes
}

#[cfg(target_os = "windows")]
fn validated_chrome_process_is_running(process: &ValidatedChromeProcess) -> bool {
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    (unsafe { WaitForSingleObject(process.handle, 0) }) == WAIT_TIMEOUT
}

#[cfg(target_os = "windows")]
unsafe fn chrome_window_class_name(window: NativeWindowHandle) -> Option<String> {
    let mut class_name = [0_u16; 256];
    let length = unsafe {
        GetClassNameW(
            window,
            class_name.as_mut_ptr(),
            i32::try_from(class_name.len()).ok()?,
        )
    };
    let length = usize::try_from(length).ok()?;
    (length > 0)
        .then(|| String::from_utf16(&class_name[..length]).ok())
        .flatten()
}

#[cfg(target_os = "windows")]
unsafe fn chrome_window_title_length(window: NativeWindowHandle) -> Option<usize> {
    const WM_GETTEXTLENGTH: u32 = 0x000e;
    const SMTO_BLOCK: u32 = 0x0001;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    const TITLE_QUERY_TIMEOUT_MS: u32 = 100;

    let mut length = 0_usize;
    (unsafe {
        SendMessageTimeoutW(
            window,
            WM_GETTEXTLENGTH,
            0,
            0,
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            TITLE_QUERY_TIMEOUT_MS,
            &mut length,
        )
    } != 0)
        .then_some(length)
}

#[cfg(target_os = "windows")]
unsafe fn chrome_window_is_onscreen(window: NativeWindowHandle) -> bool {
    const MONITOR_DEFAULTTONULL: u32 = 0;

    if unsafe { IsWindowVisible(window) } == 0 || unsafe { IsIconic(window) } != 0 {
        return false;
    }

    let mut window_rect = NativeRect::default();
    if unsafe { GetWindowRect(window, &mut window_rect) } == 0 {
        return false;
    }
    let monitor = unsafe { MonitorFromRect(&window_rect, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        return false;
    }

    let mut monitor_info = NativeMonitorInfo {
        size: std::mem::size_of::<NativeMonitorInfo>() as u32,
        monitor: NativeRect::default(),
        work: NativeRect::default(),
        flags: 0,
    };
    (unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }) != 0
        && screen_rect_visible_area_at_least(
            window_rect.into(),
            monitor_info.monitor.into(),
            CHROME_MIN_VISIBLE_WIDTH,
            CHROME_MIN_VISIBLE_HEIGHT,
        )
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn move_visible_chrome_window_callback(
    window: NativeWindowHandle,
    parameter: isize,
) -> i32 {
    let context = unsafe { &mut *(parameter as *mut VisibleWindowContext) };
    let mut pid = 0_u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    let pid_is_validated = context
        .processes
        .iter()
        .any(|process| process.pid == pid && validated_chrome_process_is_running(process));
    if !pid_is_validated {
        return 1;
    }
    let Some(class_name) = (unsafe { chrome_window_class_name(window) }) else {
        return 1;
    };
    const GW_OWNER: u32 = 4;
    let has_owner = !(unsafe { GetWindow(window, GW_OWNER) }).is_null();
    let Some(title_length) = (unsafe { chrome_window_title_length(window) }) else {
        return 1;
    };
    if !chrome_window_matches_predicate(pid_is_validated, &class_name, has_owner, title_length) {
        return 1;
    }

    const SW_RESTORE: i32 = 9;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    unsafe {
        ShowWindowAsync(window, SW_RESTORE);
    }
    let moved = unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            CHROME_VISIBLE_WINDOW_X,
            CHROME_VISIBLE_WINDOW_Y,
            CHROME_VISIBLE_WINDOW_WIDTH,
            CHROME_VISIBLE_WINDOW_HEIGHT,
            SWP_SHOWWINDOW,
        )
    } != 0;
    if moved && unsafe { chrome_window_is_onscreen(window) } {
        context.moved = true;
        unsafe {
            SetForegroundWindow(window);
        }
        return 0;
    }
    1
}

#[cfg(target_os = "windows")]
fn try_move_chrome_window_to_visible_position(processes: &[ValidatedChromeProcess]) -> bool {
    let mut context = VisibleWindowContext {
        processes,
        moved: false,
    };
    unsafe {
        EnumWindows(
            Some(move_visible_chrome_window_callback),
            (&mut context as *mut VisibleWindowContext) as isize,
        );
    }
    context.moved
}

#[cfg(target_os = "macos")]
fn try_move_chrome_window_to_visible_position(pids: &[u32]) -> bool {
    pids.iter().any(|pid| {
        let script = format!(
            "tell application \"System Events\"\nset chromeProcess to first application process whose unix id is {}\nset visible of chromeProcess to true\nset frontmost of chromeProcess to true\ntell chromeProcess\nif (count of windows) is 0 then error \"Chrome window not found\"\nset position of first window to {{{}, {}}}\nend tell\nend tell",
            pid, CHROME_VISIBLE_WINDOW_X, CHROME_VISIBLE_WINDOW_Y
        );
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

#[cfg(target_os = "linux")]
fn try_move_chrome_window_to_visible_position(pids: &[u32]) -> bool {
    if let Ok(output) = Command::new("wmctrl").arg("-lp").output()
        && output.status.success()
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 3
                && fields[2]
                    .parse::<u32>()
                    .ok()
                    .is_some_and(|pid| pids.contains(&pid))
            {
                let window = fields[0];
                let moved = Command::new("wmctrl")
                    .args([
                        "-i",
                        "-r",
                        window,
                        "-e",
                        &format!(
                            "0,{},{},-1,-1",
                            CHROME_VISIBLE_WINDOW_X, CHROME_VISIBLE_WINDOW_Y
                        ),
                    ])
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
                if moved {
                    let _ = Command::new("wmctrl").args(["-i", "-a", window]).status();
                    return true;
                }
            }
        }
    }

    for pid in pids {
        let output = Command::new("xdotool")
            .args(["search", "--pid", &pid.to_string()])
            .output();
        let Some(window) = output
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_string)
            })
        else {
            continue;
        };
        let moved = Command::new("xdotool")
            .args([
                "windowmap",
                &window,
                "windowmove",
                &window,
                &CHROME_VISIBLE_WINDOW_X.to_string(),
                &CHROME_VISIBLE_WINDOW_Y.to_string(),
                "windowactivate",
                &window,
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if moved {
            return true;
        }
    }
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn try_move_chrome_window_to_visible_position(_pids: &[u32]) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn make_chrome_window_visible(pids: &[u32]) -> Result<(), String> {
    let expected_chrome_path = find_chrome_path()?;
    let processes = open_validated_chrome_processes(pids, &expected_chrome_path);
    if processes.is_empty() {
        return Err(format!(
            "Could not validate the managed Chrome process path as {} for process(es) {}. Run `ask-bridge close` and retry.",
            expected_chrome_path,
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    for _ in 0..30 {
        if try_move_chrome_window_to_visible_position(&processes) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(format!(
        "Could not restore the managed Chrome window for process(es) {} to the visible desktop. Run `ask-bridge close` and retry.",
        pids.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(not(target_os = "windows"))]
fn make_chrome_window_visible(pids: &[u32]) -> Result<(), String> {
    // The visible launch position is the cross-platform baseline. Window-manager
    // helpers are optional on Unix, so reuse them on a best-effort basis without
    // making an otherwise visible Chrome session fail when they are unavailable.
    let _ = try_move_chrome_window_to_visible_position(pids);
    Ok(())
}

fn apply_chrome_window_mode(headless: bool, snapshot: &ChromeDebugSnapshot) -> Result<(), String> {
    apply_chrome_window_mode_with(headless, snapshot, make_chrome_window_visible)
}

fn start_chrome_if_needed(headless: bool, verbose: bool) -> Result<(), String> {
    let profile_path = chrome_profile_path()?;

    if TcpStream::connect("127.0.0.1:9223").is_ok() {
        let snapshot = inspect_chrome_debug_port(&profile_path);
        if debug_listener_scope_is_unambiguous(&snapshot.listener_pids)
            && chrome_record_matches_current(
                snapshot.record.as_ref(),
                snapshot.browser_id.as_deref(),
                &snapshot.listener_pids,
            )
        {
            if headless {
                // Force hide any existing background Chrome PIDs asynchronously just in case they are currently visible
                #[cfg(target_os = "macos")]
                {
                    let pids = snapshot.ask_pids.clone();
                    thread::spawn(move || {
                        for pid_str in pids {
                            if let Ok(pid) = pid_str.parse::<u32>() {
                                let script = format!(
                                    "tell application \"System Events\" to set visible of first application process whose unix id is {} to false",
                                    pid
                                );
                                let _ = Command::new("osascript").arg("-e").arg(&script).status();
                            }
                        }
                    });
                }
            }
            if verbose && headless && !is_debug_chrome_background(&profile_path) {
                println!(
                    "Reusing existing ask-bridge Chrome on port 9223. Run `ask-bridge close` if you want to restart it in background mode."
                );
            }
            apply_chrome_window_mode(headless, &snapshot)?;
            return Ok(());
        }

        if debug_listener_scope_is_unambiguous(&snapshot.listener_pids)
            && !snapshot.ask_pids.is_empty()
            && build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
                .is_some()
        {
            if let Some(record) =
                build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
            {
                write_chrome_process_record(&record).map_err(|error| {
                    format!("Failed to update Chrome process record: {}", error)
                })?;
            }
            if verbose {
                println!("Reusing the existing ask-bridge Chrome on port 9223.");
            }
            apply_chrome_window_mode(headless, &snapshot)?;
            return Ok(());
        }

        return Err(
            "Port 9223 is already used by a non-ask Chrome process. Stop it or use a different debugging port."
                .to_string(),
        );
    }

    if verbose {
        println!(
            "Chrome is not running on port 9223. Starting Chrome with remote debugging (headless: {})...",
            headless
        );
    }

    let chrome_path = find_chrome_path()?;
    let _ = remove_chrome_pid_file();

    let mut cmd = Command::new(&chrome_path);
    cmd.arg("--remote-debugging-port=9223")
        .arg(format!("--user-data-dir={}", profile_path))
        .arg(ASK_BRIDGE_CHROME_MARKER)
        .arg("--no-first-run")
        .arg("--no-default-browser-check");

    #[cfg(target_os = "windows")]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.args(chrome_window_launch_args(headless));

    let child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start Google Chrome: {}", e))?;

    let child_pid = child.id();

    if verbose {
        println!(
            "Started ask-bridge Chrome PID {} with profile {}.",
            child_pid, profile_path
        );
    }

    if headless {
        #[cfg(target_os = "macos")]
        {
            let pid = child.id();
            thread::spawn(move || {
                // Rapidly set visibility to false during startup to prevent window from flashing or drawing
                for _ in 0..40 {
                    let script = format!(
                        "tell application \"System Events\" to try\nset visible of first application process whose unix id is {} to false\nend try",
                        pid
                    );
                    let _ = Command::new("osascript").arg("-e").arg(&script).status();
                    thread::sleep(Duration::from_millis(50));
                }
            });
        }
    }

    let _ = child; // Avoid unused variable warning on non-macOS platforms

    // Wait for Chrome to listen and prove that the listener belongs to this launch.
    let startup_deadline = Instant::now() + Duration::from_secs(15);
    let mut last_identity_error = None;
    while Instant::now() < startup_deadline {
        if TcpStream::connect("127.0.0.1:9223").is_ok() {
            let snapshot = inspect_chrome_debug_port(&profile_path);
            if let Some(record) =
                build_chrome_process_record(&snapshot.listener_pids, snapshot.browser_id.as_deref())
            {
                if let Err(error) = write_chrome_process_record(&record) {
                    return Err(format!(
                        "Failed to record Chrome process identity: {}",
                        error
                    ));
                }
                if verbose && record.pid != child_pid {
                    println!(
                        "Recorded actual Chrome listener PID {} (launcher PID {}).",
                        record.pid, child_pid
                    );
                }
                if verbose {
                    println!("Chrome started and listening on port 9223.");
                }
                // A previously backgrounded profile can retain off-screen
                // window bounds even when this launch requested a visible
                // position. Re-apply the verified window mode after Chrome's
                // real browser process and top-level window exist; otherwise
                // VS Code-triggered login can leave only a flashing taskbar
                // thumbnail that the user cannot restore.
                apply_chrome_window_mode(headless, &snapshot)?;
                return Ok(());
            }
            last_identity_error = Some(
                "Chrome did not expose a valid CDP browser identity on port 9223.".to_string(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = remove_chrome_pid_file();
    match last_identity_error {
        Some(error) => Err(format!(
            "Failed to identify active Chrome listener: {}",
            error
        )),
        None => Err("Timed out waiting for Chrome to start on port 9223".to_string()),
    }
}

fn normalize_profile_match_text(value: &str) -> String {
    let normalized = value.replace('\\', "/").replace(['"', '\''], "");

    #[cfg(target_os = "windows")]
    {
        normalized.to_ascii_lowercase()
    }

    #[cfg(not(target_os = "windows"))]
    {
        normalized
    }
}

fn command_has_argument(command: &str, argument: &str) -> bool {
    command.match_indices(argument).any(|(start, matched)| {
        let before_is_boundary = start == 0
            || command[..start]
                .chars()
                .next_back()
                .map(char::is_whitespace)
                .unwrap_or(false);
        let end = start + matched.len();
        let after_is_boundary = end == command.len()
            || command[end..]
                .chars()
                .next()
                .map(char::is_whitespace)
                .unwrap_or(false);
        before_is_boundary && after_is_boundary
    })
}

fn command_uses_profile(command: &str, profile_path: &str) -> bool {
    let command = normalize_profile_match_text(command);
    let profile_path = normalize_profile_match_text(profile_path);

    command_has_argument(&command, &format!("--user-data-dir={}", profile_path))
        || command_has_argument(&command, &format!("--user-data-dir {}", profile_path))
}

fn command_identifies_ask_chrome(command: &str, profile_path: &str) -> bool {
    command_uses_profile(command, profile_path)
        || command_has_argument(command, ASK_BRIDGE_CHROME_MARKER)
}

fn find_ask_chrome_owner_pid_with<C, P>(
    listener_pid: &str,
    profile_path: &str,
    mut command_for: C,
    mut parent_for: P,
) -> Option<String>
where
    C: FnMut(&str) -> Option<String>,
    P: FnMut(&str) -> Option<String>,
{
    let mut current_pid = listener_pid.to_string();

    for _ in 0..16 {
        if command_for(&current_pid)
            .map(|command| command_identifies_ask_chrome(&command, profile_path))
            .unwrap_or(false)
        {
            return Some(current_pid);
        }

        let parent_pid = parent_for(&current_pid)?;
        if parent_pid.is_empty() || parent_pid == "0" || parent_pid == current_pid {
            return None;
        }
        current_pid = parent_pid;
    }

    None
}

fn chrome_record_matches_browser(record: &ChromeProcessRecord, browser_id: Option<&str>) -> bool {
    matches!(
        (record.browser_id.as_deref(), browser_id),
        (Some(recorded_id), Some(current_id)) if recorded_id == current_id
    )
}

fn chrome_record_matches_current(
    record: Option<&ChromeProcessRecord>,
    browser_id: Option<&str>,
    listener_pids: &[String],
) -> bool {
    record.is_some_and(|record| {
        chrome_record_matches_browser(record, browser_id)
            && listener_pids.len() == 1
            && listener_pids[0] == record.pid.to_string()
    })
}

fn find_ask_chrome_owner_pids_with<C, P>(
    listener_pids: &[String],
    profile_path: &str,
    mut command_for: C,
    mut parent_for: P,
) -> Vec<String>
where
    C: FnMut(&str) -> Option<String>,
    P: FnMut(&str) -> Option<String>,
{
    let mut ask_pids = Vec::new();
    for listener_pid in listener_pids {
        let ask_pid = find_ask_chrome_owner_pid_with(
            listener_pid,
            profile_path,
            &mut command_for,
            &mut parent_for,
        );

        if let Some(ask_pid) = ask_pid
            && !ask_pids.contains(&ask_pid)
        {
            ask_pids.push(ask_pid);
        }
    }
    ask_pids
}

struct ChromeDebugSnapshot {
    listener_pids: Vec<String>,
    record: Option<ChromeProcessRecord>,
    browser_id: Option<String>,
    ask_pids: Vec<String>,
}

fn debug_listener_scope_is_unambiguous(listener_pids: &[String]) -> bool {
    listener_pids.len() <= 1
}

fn inspect_chrome_debug_port(profile_path: &str) -> ChromeDebugSnapshot {
    let listener_pids = debug_port_listener_pids();
    let record = read_chrome_process_record();
    let browser_id = debug_browser_id();
    let ask_pids = find_ask_chrome_owner_pids_with(
        &listener_pids,
        profile_path,
        process_command,
        process_parent_pid,
    );
    ChromeDebugSnapshot {
        listener_pids,
        record,
        browser_id,
        ask_pids,
    }
}

fn ask_chrome_pids_on_debug_port(profile_path: &str) -> Vec<String> {
    inspect_chrome_debug_port(profile_path).ask_pids
}

#[cfg(target_os = "windows")]
fn parse_windows_netstat_listener_pids(output: &str, port: u16) -> Vec<String> {
    let mut pids = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5
            || !fields[0].eq_ignore_ascii_case("TCP")
            || !fields[3].eq_ignore_ascii_case("LISTENING")
            || fields[1]
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse::<u16>().ok())
                != Some(port)
        {
            continue;
        }

        let pid = fields[4];
        if pid.chars().all(|character| character.is_ascii_digit())
            && !pids.iter().any(|existing| existing == pid)
        {
            pids.push(pid.to_string());
        }
    }
    pids
}

fn debug_port_listener_pids() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("netstat").args(["-ano", "-p", "tcp"]).output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_windows_netstat_listener_pids(&stdout, 9223)
            }
            _ => Vec::new(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("lsof")
            .args(["-tiTCP:9223", "-sTCP:LISTEN"])
            .output();

        match output {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(target_os = "windows")]
fn parse_wmic_column_value(output: &str) -> Option<String> {
    let mut non_empty_lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    non_empty_lines.next()?;
    non_empty_lines.next().map(str::to_string)
}

fn process_command(pid: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("processid={}", pid),
                "get",
                "commandline",
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(command) = parse_wmic_column_value(&stdout) {
                return Some(command);
            }
        }

        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter 'ProcessId = {}').CommandLine",
                    pid
                ),
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !stdout.is_empty() {
                return Some(stdout);
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", pid, "-o", "command="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn process_parent_pid(pid: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("processid={}", pid),
                "get",
                "parentprocessid",
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
            && let Some(parent_pid) = parse_wmic_column_value(&String::from_utf8_lossy(&out.stdout))
        {
            return Some(parent_pid);
        }

        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter 'ProcessId = {}').ParentProcessId",
                    pid
                ),
            ])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let parent_pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !parent_pid.is_empty() {
                return Some(parent_pid);
            }
        }

        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", pid, "-o", "ppid="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let parent_pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if parent_pid.is_empty() {
            None
        } else {
            Some(parent_pid)
        }
    }
}

fn is_debug_chrome_background(profile_path: &str) -> bool {
    ask_chrome_pids_on_debug_port(profile_path)
        .iter()
        .any(|pid| {
            process_command(pid)
                .map(|cmd| cmd.contains("--ask-bridge-background"))
                .unwrap_or(false)
        })
}

fn close_ask_chrome_on_debug_port(profile_path: &str) -> Result<bool, String> {
    let snapshot = inspect_chrome_debug_port(profile_path);
    if snapshot.listener_pids.is_empty() {
        if TcpStream::connect("127.0.0.1:9223").is_ok() {
            return Err(
                "Port 9223 is active, but ask-bridge could not identify its listener process. No process was closed."
                    .to_string(),
            );
        }
        if let Err(_error) = remove_chrome_pid_file() {
            // ignore cleanup failure when port is already closed
        }
        return Ok(false);
    }
    if !debug_listener_scope_is_unambiguous(&snapshot.listener_pids) {
        return Err(
            "Multiple processes are listening on port 9223, so ask-bridge cannot safely determine which process to close. No process was closed."
                .to_string(),
        );
    }

    if snapshot.ask_pids.is_empty() {
        return Err(
            "Port 9223 is already used by a non-ask Chrome process. Stop it or use a different debugging port."
                .to_string(),
        );
    }

    for pid in &snapshot.ask_pids {
        #[cfg(target_os = "windows")]
        {
            let _ = Command::new("taskkill").args(["/PID", pid, "/T"]).status();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = Command::new("kill").args(["-TERM", pid]).status();
        }
    }

    for _ in 0..50 {
        if TcpStream::connect("127.0.0.1:9223").is_err() {
            let _ = remove_chrome_pid_file();
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err("Timed out waiting for existing ask-bridge Chrome to stop".to_string())
}

static FORWARD_MCP_STDERR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// One MCP session per run: a single long-lived chrome-devtools-mcp child plus
/// the tokio runtime that drives its background reader tasks.
///
/// Upstream called `McpClient::call_tool` per browser action, which spawns a
/// fresh `npx chrome-devtools-mcp` child for every single action (~50 per
/// query) and waits on its response without any timeout — one stalled npx
/// spawn hung the whole run forever (2026-07-11). Reusing one connection
/// removes the re-spawn churn; `MCP_CALL_TIMEOUT` turns any remaining stall
/// into a loud, bounded error (see `mcp_error_is_transport` for why the failed
/// call is not replayed).
struct McpSession {
    connection: McpConnection,
    runtime: tokio::runtime::Runtime,
    config_path: String,
}

static MCP_SESSION: std::sync::Mutex<Option<McpSession>> = std::sync::Mutex::new(None);

const MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(90);
const MCP_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

fn mcp_session_connect(config_path: &str) -> Result<McpSession, String> {
    let client = McpClient::load(Some(config_path))
        .map_err(|e| format!("Failed to load MCP config: {}", e))?;
    let server_config = client
        .server_config("chrome-devtools")
        .map_err(|e| format!("Missing chrome-devtools MCP server config: {}", e))?;
    // A multi-thread runtime with one worker keeps the connection's background
    // stdout/stderr reader tasks running between calls (a current-thread
    // runtime only makes progress inside block_on).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to create async runtime for MCP session: {}", e))?;
    let connection = runtime.block_on(async {
        // Connect the stdio transport directly: mcp-cli's default path first
        // tries its persistent daemon, which re-execs this binary with
        // `--daemon` — an entrypoint ask-bridge does not implement — so that
        // path can only ever fail and fall back.
        let connect_future = async {
            match &server_config {
                ServerConfig::Stdio(stdio_config) => {
                    StdioClient::connect("chrome-devtools", stdio_config)
                        .await
                        .map(McpConnection::Stdio)
                }
                _ => client.connect("chrome-devtools").await,
            }
        };
        match tokio::time::timeout(MCP_CONNECT_TIMEOUT, connect_future).await {
            Err(_) => Err(format!(
                "Failed to start chrome-devtools MCP server: timed out after {}s",
                MCP_CONNECT_TIMEOUT.as_secs()
            )),
            Ok(result) => {
                result.map_err(|e| format!("Failed to start chrome-devtools MCP server: {}", e))
            }
        }
    })?;
    Ok(McpSession {
        connection,
        runtime,
        config_path: config_path.to_string(),
    })
}

fn mcp_session_reset(slot: &mut Option<McpSession>) {
    if let Some(session) = slot.take() {
        let McpSession {
            connection,
            runtime,
            ..
        } = session;
        // Best-effort close (kills the child); if even that stalls, dropping
        // the runtime stops the background tasks and the orphaned child exits
        // on stdin EOF.
        let _ = runtime
            .block_on(async { tokio::time::timeout(MCP_CLOSE_TIMEOUT, connection.close()).await });
    }
}

fn mcp_session_call(
    slot: &mut Option<McpSession>,
    config_path: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    let needs_connect = slot
        .as_ref()
        .map(|session| session.config_path != config_path)
        .unwrap_or(true);
    if needs_connect {
        mcp_session_reset(slot);
        *slot = Some(mcp_session_connect(config_path)?);
    }
    let session = slot.as_ref().expect("session connected above");
    session.runtime.block_on(async {
        match tokio::time::timeout(MCP_CALL_TIMEOUT, session.connection.call_tool(tool, args)).await
        {
            Err(_) => Err(format!(
                "MCP tool '{}' timed out after {}s",
                tool,
                MCP_CALL_TIMEOUT.as_secs()
            )),
            Ok(result) => result.map_err(|e| format!("mcp-cli library call failed: {}", e)),
        }
    })
}

/// Errors that mean the MCP transport itself is dead or wedged: our own
/// timeouts, or transport-level failures (dead child / closed pipes — exact
/// phrases from mcp-cli's StdioClient). These earn a session reset so the next
/// command starts clean. The failed call is deliberately NOT replayed: a
/// timed-out request may already have executed in the browser (replaying a
/// submit would double-post), and a fresh chrome-devtools-mcp child forgets
/// the selected page (a replay could act on the wrong tab). Application-level
/// tool errors (e.g. a JS exception from evaluate_script) propagate unchanged.
fn mcp_error_is_transport(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("timed out")
        || lower.contains("failed to send request to process stdin")
        || lower.contains("server process exited unexpectedly")
        || lower.contains("stdio response receiver canceled")
        || lower.contains("failed to start chrome-devtools mcp server")
}

fn call_mcp_tool(config_path: &str, tool: &str, args: Value) -> Result<Value, String> {
    let _stderr_guard = if FORWARD_MCP_STDERR.load(std::sync::atomic::Ordering::Relaxed) {
        None
    } else {
        Some(
            gag::Gag::stderr()
                .map_err(|e| format!("Failed to suppress MCP stderr in quiet mode: {}", e))?,
        )
    };

    let mut slot = MCP_SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match mcp_session_call(&mut slot, config_path, tool, args) {
        Ok(value) => Ok(value),
        Err(error) => {
            if mcp_error_is_transport(&error) {
                mcp_session_reset(&mut slot);
                return Err(format!(
                    "{} (MCP session was reset; re-run the command)",
                    error
                ));
            }
            Err(error)
        }
    }
}

fn parse_pages(text: &str) -> Vec<Page> {
    let mut pages = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("##") {
            continue;
        }
        if let Some((id_str, rest)) = line.split_once(':') {
            let id = match id_str.trim().parse::<usize>() {
                Ok(id) => id,
                Err(_) => continue,
            };
            let rest = rest.trim();
            let (url, selected) = if rest.ends_with("[selected]") {
                let url = rest.strip_suffix("[selected]").unwrap().trim().to_string();
                (url, true)
            } else {
                (rest.to_string(), false)
            };
            pages.push(Page { id, url, selected });
        }
    }
    pages
}

fn parse_script_result(val: &Value) -> Result<Value, String> {
    let text = val
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Could not extract text field from evaluate_script result".to_string())?;

    let start_tag = "```json";

    if let Some(start_pos) = text.find(start_tag) {
        let json_start = start_pos + start_tag.len();
        let json_str = text[json_start..].trim_start();
        let mut values = serde_json::Deserializer::from_str(json_str).into_iter::<Value>();
        let parsed = values
            .next()
            .ok_or_else(|| "JSON parsing error: missing JSON value".to_string())?
            .map_err(|e| format!("JSON parsing error: {}", e))?;
        let remainder = json_str[values.byte_offset()..].trim_start();
        let after_fence = remainder
            .strip_prefix("```")
            .ok_or_else(|| "Could not find closing JSON fence in script result".to_string())?;
        if !matches!(after_fence.chars().next(), None | Some('\r') | Some('\n')) {
            return Err("Invalid closing JSON fence in script result".to_string());
        }
        return Ok(parsed);
    }

    Err("Could not find JSON fencing in script result".to_string())
}

fn tool_text(val: &Value) -> Result<String, String> {
    val.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .map(|text| text.to_string())
        .ok_or_else(|| format!("Could not extract text field from tool result: {:?}", val))
}

fn take_snapshot_text(config_path: &str) -> Result<String, String> {
    let res = call_mcp_tool(config_path, "take_snapshot", serde_json::json!({}))?;
    tool_text(&res)
}

fn extract_snapshot_uid(line: &str) -> Option<String> {
    let marker_pos = line.find("uid=")?;
    let mut rest = line[marker_pos + 4..].trim_start();
    rest = rest.trim_start_matches(['"', '\'', '[']);
    let uid: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ']')
        .collect();
    if uid.is_empty() { None } else { Some(uid) }
}

fn find_snapshot_uid(snapshot: &str, include: &[&str], exclude: &[&str]) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let lower = line.to_lowercase();
        let includes_all = include
            .iter()
            .all(|needle| lower.contains(&needle.to_lowercase()));
        let excludes_all = exclude
            .iter()
            .all(|needle| !lower.contains(&needle.to_lowercase()));
        if includes_all && excludes_all {
            extract_snapshot_uid(line)
        } else {
            None
        }
    })
}

fn find_copilot_add_content_uid(snapshot: &str) -> Option<String> {
    let exclude = [
        "work content",
        "cloud",
        "onedrive",
        "工作內容",
        "工作内容",
        "雲端",
        "云端",
    ];

    find_snapshot_uid(snapshot, &["add", "content"], &exclude)
        .or_else(|| find_snapshot_uid(snapshot, &["新增", "內容"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["添加", "内容"], &exclude))
}

fn find_copilot_upload_images_and_files_uid(snapshot: &str) -> Option<String> {
    let exclude = [
        "work content",
        "cloud",
        "onedrive",
        "工作內容",
        "工作内容",
        "雲端",
        "云端",
    ];

    find_snapshot_uid(snapshot, &["upload", "images", "files"], &exclude)
        .or_else(|| find_snapshot_uid(snapshot, &["upload", "image", "file"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["upload", "file"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上傳", "圖片", "檔案"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上傳", "影像", "檔案"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上傳", "檔案"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上传", "图像", "文件"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上传", "图片", "文件"], &exclude))
        .or_else(|| find_snapshot_uid(snapshot, &["上传", "文件"], &exclude))
}

fn is_glow_available() -> bool {
    Command::new("glow")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn render_markdown(markdown: &str, use_glow: bool) -> Result<(), String> {
    if markdown.is_empty() {
        return Ok(());
    }

    if use_glow {
        let glow = Command::new("glow")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();

        if let Ok(mut child) = glow {
            let stdin_opt = child.stdin.take();
            if let Some(mut stdin) = stdin_opt {
                let _ = stdin.write_all(markdown.as_bytes()).map_err(|e| {
                    eprintln!("Failed to send Markdown content to glow: {}", e);
                });
            }

            match child.wait() {
                Ok(status) if status.success() => {
                    return Ok(());
                }
                Ok(status) => {
                    eprintln!("glow exited with status: {}", status);
                }
                Err(e) => {
                    eprintln!("Failed to wait for glow process: {}", e);
                }
            }
        }
    }

    print!("{}", markdown);
    io::stdout()
        .flush()
        .map_err(|e| format!("Failed to flush stdout: {}", e))?;

    Ok(())
}

fn validate_provider_feature_support(provider: Provider, cli: &Cli) -> Result<(), String> {
    if provider == Provider::Gemini && !cli.images.is_empty() {
        return Err(
            "Gemini image attachments are not supported yet. Use --file for Gemini document attachments."
                .to_string(),
        );
    }

    if provider == Provider::Copilot && cli.model.is_some() {
        return Err("Microsoft 365 Copilot model switching is not supported yet.".to_string());
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn validates_chrome_devtools_mcp_node_versions() {
        for version in [
            "v20.19.0",
            "v20.20.1\r\n",
            "v22.12.0",
            "v22.15.1",
            "v23.0.0",
            "v24.4.1",
        ] {
            assert!(
                validate_node_version_output(version).is_ok(),
                "expected {version:?} to be supported"
            );
        }

        for version in ["v18.20.8", "v20.17.0", "v20.18.9", "v21.7.3", "v22.11.0"] {
            assert!(
                validate_node_version_output(version).is_err(),
                "expected {version:?} to be rejected"
            );
        }
    }

    #[test]
    fn reports_actionable_node_version_errors() {
        let unsupported = validate_node_version_output("v20.17.0").unwrap_err();
        assert!(unsupported.contains("v20.17.0"));
        assert!(unsupported.contains("^20.19.0"));
        assert!(unsupported.contains("reopen the terminal"));

        for output in ["", "20.19", "not-a-version", "v20.19.0.1"] {
            assert!(
                validate_node_version_output(output).is_err(),
                "expected {output:?} to be rejected"
            );
        }
    }

    #[test]
    fn pins_chrome_devtools_mcp_version() {
        // `@latest` makes every npx spawn re-resolve the dist-tag against the
        // npm registry; combined with mcp-cli's timeout-less request wait this
        // hung whole runs (2026-07-11). The package spec must pin a version.
        let config = build_chrome_devtools_server_config(true, true, "/tmp/mcp.log", false);
        let args = config["args"].as_array().expect("args array");
        let pkg = args
            .iter()
            .filter_map(|a| a.as_str())
            .find(|a| a.starts_with("chrome-devtools-mcp"))
            .expect("chrome-devtools-mcp package argument");
        assert!(
            !pkg.ends_with("@latest"),
            "chrome-devtools-mcp must be version-pinned, got {pkg}"
        );
        let version = pkg.rsplit('@').next().unwrap_or_default();
        assert!(
            version.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "expected an explicit pinned version, got {pkg}"
        );
    }

    #[test]
    fn classifies_transport_errors_for_reconnect() {
        // Transport failures earn a session reset + loud error (exact phrases
        // from mcp-cli's StdioClient surface inside CliError's `Details:`
        // line); the call is never replayed — see mcp_error_is_transport...
        for transport in [
            "MCP tool 'click' timed out after 90s",
            "Error [SERVER_CONNECTION_FAILED]: x\n  Details: Failed to send request to process stdin",
            "Error [TOOL_EXECUTION_FAILED]: x\n  Details: Server process exited unexpectedly. Last stderr:\nnpm error",
            "Error [SERVER_CONNECTION_FAILED]: x\n  Details: Stdio response receiver canceled",
            "Failed to start chrome-devtools MCP server: timed out after 120s",
        ] {
            assert!(
                mcp_error_is_transport(transport),
                "expected transport-class error: {transport}"
            );
        }
        // ...application-level tool errors must NOT reset the session — the
        // transport is fine and the caller needs the original error.
        for app_level in [
            "mcp-cli library call failed: Error [TOOL_EXECUTION_FAILED]: Tool \"click\" execution failed\n  Details: element not found",
            "mcp-cli library call failed: Error [TOOL_EXECUTION_FAILED]: Tool \"evaluate_script\" execution failed\n  Details: TypeError: x is undefined",
        ] {
            assert!(
                !mcp_error_is_transport(app_level),
                "expected app-level error to pass through: {app_level}"
            );
        }
    }

    #[test]
    fn piped_stdin_grace_skips_silent_pipe_when_prompt_argument_present() {
        // Agent harnesses (Claude Code / Codex) run commands with a non-tty
        // stdin they may never close; blocking on EOF hung whole runs
        // (2026-07-11). With a prompt argument in hand, a silent pipe must be
        // treated as "no piped input" after the grace period.
        let (_probe_tx, probe_rx) = std::sync::mpsc::channel::<StdinProbe>();
        let (_data_tx, data_rx) = std::sync::mpsc::channel::<std::io::Result<String>>();
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(50), true)
            .expect("silent pipe should yield empty stdin, not an error");
        assert_eq!(out, "");
    }

    #[test]
    fn piped_stdin_reads_live_pipe_to_eof_when_prompt_argument_present() {
        // A pipe that delivers data keeps the documented combine behavior:
        // `cat notes.md | ask-bridge '摘要'` must still append stdin.
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (data_tx, data_rx) = std::sync::mpsc::channel();
        probe_tx.send(StdinProbe::Data).unwrap();
        data_tx.send(Ok("piped context".to_string())).unwrap();
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(50), true)
            .expect("live pipe should be read");
        assert_eq!(out, "piped context");
    }

    #[test]
    fn piped_stdin_waits_unbounded_when_no_prompt_argument() {
        // Without a prompt argument stdin IS the prompt: keep upstream's
        // unbounded wait even when data arrives long after any grace window.
        let (_probe_tx, probe_rx) = std::sync::mpsc::channel();
        let (data_tx, data_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            let _ = data_tx.send(Ok("stdin is the prompt".to_string()));
        });
        let out = recv_piped_stdin(&probe_rx, &data_rx, Duration::from_millis(10), false)
            .expect("unbounded wait should return the piped prompt");
        assert_eq!(out, "stdin is the prompt");
    }

    #[test]
    fn builds_direct_quiet_mcp_configs() {
        fn config_args(config: &serde_json::Value) -> Vec<&str> {
            config["args"]
                .as_array()
                .expect("MCP config should contain an args array")
                .iter()
                .map(|arg| arg.as_str().expect("MCP arguments should be strings"))
                .collect()
        }

        let log_path = r"C:\Temp\ask bridge\chrome-devtools-mcp.log";
        let quiet_windows = build_chrome_devtools_server_config(true, true, log_path, true);
        let verbose_windows = build_chrome_devtools_server_config(false, true, log_path, true);
        let quiet_unix = build_chrome_devtools_server_config(true, true, log_path, false);
        let quiet_args = config_args(&quiet_windows);
        let verbose_args = config_args(&verbose_windows);

        assert_eq!(quiet_windows["command"].as_str(), Some("npx.cmd"));
        assert_eq!(verbose_windows["command"].as_str(), Some("npx.cmd"));
        assert_eq!(quiet_unix["command"].as_str(), Some("npx"));
        for required in [
            MCP_PACKAGE_SPEC,
            "--browser-url=http://127.0.0.1:9223",
            "--headless",
            "--logFile",
            log_path,
        ] {
            assert!(quiet_args.contains(&required));
            assert!(verbose_args.contains(&required));
        }
        assert!(quiet_args.contains(&"--no-usage-statistics"));
        assert!(quiet_args.contains(&"--no-performance-crux"));
        assert!(!verbose_args.contains(&"--no-usage-statistics"));
        assert!(!verbose_args.contains(&"--no-performance-crux"));
        assert!(!quiet_args.iter().any(|arg| arg.contains("2>nul")));
        assert_eq!(quiet_windows["env"]["CI"].as_str(), Some("1"));
        assert!(verbose_windows.get("env").is_none());
    }

    #[test]
    fn parses_script_result_containing_markdown_code_fence() {
        let markdown = "說明\n```rust\nfn main() { println!(\"ok\"); }\n```\n結尾";
        let encoded = serde_json::to_string(markdown).expect("markdown should serialize");
        let result = serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!("Script ran on page and returned:\n```json\n{}\n```", encoded)
            }]
        });

        assert_eq!(
            parse_script_result(&result).expect("script result should parse"),
            serde_json::Value::String(markdown.to_string())
        );
    }

    #[test]
    fn rejects_malformed_script_fence_without_leaking_payload() {
        let secret = "private-response-content";
        let encoded = serde_json::to_string(secret).expect("secret should serialize");

        for text in [
            format!("Script ran on page and returned:\n```json\n{}", encoded),
            format!(
                "Script ran on page and returned:\n```json\n{} trailing-data\n```",
                encoded
            ),
        ] {
            let result = serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            });
            let error = parse_script_result(&result).expect_err("malformed fence should fail");

            assert!(!error.contains(secret));
        }
    }

    #[test]
    fn rejects_malformed_script_shape_without_leaking_payload() {
        let secret = "private-response-content";
        let result = serde_json::json!({
            "content": [{ "type": "text", "unexpected": secret }]
        });
        let error = parse_script_result(&result).expect_err("malformed shape should fail");

        assert!(!error.contains(secret));
        assert!(error.contains("Could not extract text field"));
    }

    fn make_test_dir(name: &str) -> std::path::PathBuf {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ask_bridge_{}_{}_{}",
            name,
            std::process::id(),
            timestamp
        ))
    }

    #[test]
    fn parses_provider_as_global_argument() {
        let cli = Cli::try_parse_from(["ask-bridge", "--provider", "gemini", "login"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Login)));

        let cli = Cli::try_parse_from(["ask-bridge", "login", "--provider", "gemini"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Login)));
    }

    #[test]
    fn parses_config_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "config", "--provider", "gemini"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Gemini));
        assert!(matches!(cli.command, Some(Commands::Config)));
    }

    #[test]
    fn parses_config_command_without_provider() {
        let cli = Cli::try_parse_from(["ask-bridge", "config"]).unwrap();
        assert_eq!(cli.provider, None);
        assert!(matches!(cli.command, Some(Commands::Config)));
    }

    #[test]
    fn parses_update_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "update"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Update)));
    }

    #[test]
    fn leaves_provider_unset_when_cli_argument_is_missing() {
        let cli = Cli::try_parse_from(["ask-bridge", "hello"]).unwrap();
        assert_eq!(cli.provider, None);
    }

    #[test]
    fn parses_provider_from_config_json() {
        assert_eq!(
            parse_configured_provider(r#"{"provider":"gemini"}"#).unwrap(),
            Some(Provider::Gemini)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"chatgpt"}"#).unwrap(),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"chat-gpt"}"#).unwrap(),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"claude"}"#).unwrap(),
            Some(Provider::Claude)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"claude-ai"}"#).unwrap(),
            Some(Provider::Claude)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"copilot"}"#).unwrap(),
            Some(Provider::Copilot)
        );
        assert_eq!(
            parse_configured_provider(r#"{"provider":"m365-copilot"}"#).unwrap(),
            Some(Provider::Copilot)
        );
        assert_eq!(parse_configured_provider(r#"{}"#).unwrap(), None);
    }

    #[test]
    fn resolves_provider_precedence() {
        assert_eq!(
            effective_provider(Some(Provider::ChatGpt), Some(Provider::Gemini)),
            Provider::ChatGpt
        );
        assert_eq!(
            effective_provider(None, Some(Provider::Gemini)),
            Provider::Gemini
        );
        assert_eq!(effective_provider(None, None), Provider::ChatGpt);
    }

    #[test]
    fn cli_provider_bypasses_invalid_config() {
        let provider = resolve_provider_with(Some(Provider::ChatGpt), || {
            Err("config should not be loaded".to_string())
        })
        .unwrap();

        assert_eq!(provider, Provider::ChatGpt);
    }

    #[test]
    fn resolves_provider_from_config_when_cli_provider_is_missing() {
        let provider = resolve_provider_with(None, || Ok(Some(Provider::Gemini))).unwrap();
        assert_eq!(provider, Provider::Gemini);
    }

    #[test]
    fn rejects_invalid_provider_in_config_json() {
        let err = parse_configured_provider(r#"{"provider":"unknown"}"#).unwrap_err();
        assert!(err.contains("Invalid provider"));
    }

    #[test]
    fn parses_close_command() {
        let cli = Cli::try_parse_from(["ask-bridge", "close"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Close)));
    }

    #[test]
    fn hides_debug_commands_from_help() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        assert!(!help.contains("\n  open"));
        assert!(!help.contains("\n  get"));
        assert!(!help.contains("\n  dump"));
        assert!(!help.contains("\n  screenshot"));
        assert!(help.contains("\n  login"));
        assert!(help.contains("\n  close"));
        assert!(help.contains("\n  update"));
    }

    #[test]
    fn still_parses_hidden_debug_commands() {
        let cli = Cli::try_parse_from(["ask-bridge", "open"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Open { .. })));

        let cli = Cli::try_parse_from(["ask-bridge", "get"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Get { .. })));

        let cli = Cli::try_parse_from(["ask-bridge", "dump"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Dump)));

        let cli = Cli::try_parse_from(["ask-bridge", "screenshot"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Screenshot)));
    }

    #[test]
    fn parses_verbose_get_command_flag() {
        let url = "https://chatgpt.com/c/6a50fe34-43c0-83ee-ab86-d41adf91625e";
        let cli = Cli::try_parse_from(["ask-bridge", "get", "--verbose", url]).unwrap();
        if let Some(Commands::Get {
            url: parsed_url,
            verbose,
        }) = cli.command
        {
            assert_eq!(parsed_url, Some(url.to_string()));
            assert!(verbose);
        } else {
            panic!("expected get command");
        }
        assert!(!cli.verbose);
    }

    #[test]
    fn parses_copilot_provider_argument() {
        let cli = Cli::try_parse_from(["ask-bridge", "--provider", "copilot", "hello"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Copilot));
    }

    #[test]
    fn parses_claude_provider_argument() {
        let cli = Cli::try_parse_from(["ask-bridge", "--provider", "claude", "hello"]).unwrap();
        assert_eq!(cli.provider, Some(Provider::Claude));
    }

    #[test]
    fn maps_provider_urls() {
        assert_eq!(
            Provider::from_url("https://chatgpt.com/c/abc"),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            Provider::from_url("https://gemini.google.com/app/abc"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::from_url("https://claude.ai/chat/abc"),
            Some(Provider::Claude)
        );
        assert_eq!(
            Provider::from_url("https://m365.cloud.microsoft/chat/abc"),
            Some(Provider::Copilot)
        );
        assert_eq!(Provider::from_url("https://example.com"), None);
    }

    #[test]
    fn copilot_selector_lists_are_valid_json() {
        for selectors in [
            Provider::Copilot.composer_selectors_json(),
            Provider::Copilot.send_button_selectors_json(),
            Provider::Copilot.stop_button_selectors_json(),
        ] {
            let parsed: Vec<String> = serde_json::from_str(selectors).unwrap();
            assert!(!parsed.is_empty());
        }
    }

    #[test]
    fn copilot_selectors_match_current_localized_m365_controls() {
        let send_selectors: Vec<String> =
            serde_json::from_str(Provider::Copilot.send_button_selectors_json()).unwrap();
        assert!(send_selectors.contains(&"button[type=\"submit\"][aria-label]".to_string()));
        assert!(
            send_selectors
                .iter()
                .any(|selector| selector.contains("SendButton"))
        );
        assert!(
            send_selectors
                .iter()
                .any(|selector| selector.contains("傳送"))
        );

        let stop_selectors: Vec<String> =
            serde_json::from_str(Provider::Copilot.stop_button_selectors_json()).unwrap();
        assert!(
            stop_selectors
                .iter()
                .any(|selector| selector.contains("停止"))
        );

        let response_selector = Provider::Copilot.latest_response_selector();
        assert!(response_selector.contains(".fai-CopilotMessage"));
        assert!(!response_selector.contains("[class*=\"CopilotMessage\"]"));
    }

    #[test]
    fn copilot_login_signals_recognize_visible_localized_sign_in_controls() {
        let ready_script = Provider::Copilot.ready_check_js();
        let login_script = Provider::Copilot.login_signals_js();

        for label in ["登入", "登錄", "登录"] {
            assert!(ready_script.contains(label));
            assert!(login_script.contains(label));
        }
        assert!(ready_script.contains(".some((el) => isVisible(el)"));
        assert!(login_script.contains("controls.find((el) => isVisible(el)"));

        let signals = LoginSignals {
            account: false,
            auth_control: true,
            auth_path: false,
            composer: false,
            stable: true,
        };
        assert_eq!(signals.state(Provider::Copilot), LoginState::LoggedOut);
    }

    #[test]
    fn copilot_login_signals_recognize_visible_localized_account_controls() {
        let script = Provider::Copilot.login_signals_js();

        for label in ["登出", "退出登錄", "退出登录", "帳戶管理", "帐户管理"] {
            assert!(script.contains(label));
        }
        assert!(script.contains("if (!isVisible(el)) return false"));

        let signals = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: false,
            composer: false,
            stable: true,
        };
        assert_eq!(signals.state(Provider::Copilot), LoginState::LoggedIn);
    }

    #[test]
    fn trusted_composer_clear_uses_platform_select_all_shortcuts() {
        assert_eq!(select_all_shortcut(false), "Control+A");
        assert_eq!(select_all_shortcut(true), "Meta+A");
    }

    #[test]
    fn allows_copilot_image_and_file_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "copilot",
            "--image",
            "screen.png",
            "--image",
            "detail.jpg",
            "hello",
            "--file",
            "notes.md",
            "--file",
            "src/main.rs",
        ])
        .unwrap();
        assert_eq!(cli.images, ["screen.png", "detail.jpg"]);
        assert_eq!(cli.files, ["notes.md", "src/main.rs"]);
        assert!(validate_provider_feature_support(Provider::Copilot, &cli).is_ok());
    }

    #[test]
    fn rejects_copilot_model_switching() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "copilot",
            "hello",
            "--model",
            "work",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Copilot, &cli).is_err());
    }

    #[test]
    fn parses_chatgpt_agent_prompt_with_chinese_agent_name() {
        assert_eq!(
            parse_chatgpt_agent_prompt(
                "@智慧 研究多奇數位創意有限公司的發展沿革與創辦人的豐功偉業"
            ),
            Some(ChatGptAgentPrompt {
                agent_mention: "@智慧",
                body: "研究多奇數位創意有限公司的發展沿革與創辦人的豐功偉業"
            })
        );
    }

    #[test]
    fn parses_chatgpt_agent_prompt_with_ten_character_agent_name() {
        assert_eq!(
            parse_chatgpt_agent_prompt("@一二三四五六七八九十 查資料"),
            Some(ChatGptAgentPrompt {
                agent_mention: "@一二三四五六七八九十",
                body: "查資料"
            })
        );
    }

    #[test]
    fn trims_extra_whitespace_between_chatgpt_agent_and_body() {
        assert_eq!(
            parse_chatgpt_agent_prompt("@智慧 \n\t查資料").unwrap().body,
            "查資料"
        );
    }

    #[test]
    fn rejects_invalid_chatgpt_agent_prompt_shapes() {
        assert_eq!(parse_chatgpt_agent_prompt("智慧 查資料"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@ 查資料"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@智慧"), None);
        assert_eq!(parse_chatgpt_agent_prompt("@智慧   "), None);
        assert_eq!(
            parse_chatgpt_agent_prompt("@一二三四五六七八九十甲 查資料"),
            None
        );
    }

    #[test]
    fn extracts_snapshot_uid_from_common_formats() {
        assert_eq!(
            extract_snapshot_uid(r#"- button "上傳檔案" [uid="1_23"]"#),
            Some("1_23".to_string())
        );
        assert_eq!(
            extract_snapshot_uid(r#"- button "Upload file" uid=42"#),
            Some("42".to_string())
        );
    }

    #[test]
    fn finds_snapshot_uid_with_include_and_exclude_terms() {
        let snapshot = r#"
            - button "加入雲端硬碟檔案" [uid="1_10"]
            - menuitem "上傳檔案. 文件、資料、程式碼檔案" [uid="1_11"]
        "#;
        assert_eq!(
            find_snapshot_uid(snapshot, &["上傳檔案"], &["雲端"]),
            Some("1_11".to_string())
        );
    }

    #[test]
    fn finds_copilot_attachment_controls_in_english_snapshot() {
        let snapshot = r#"
            - button "Add content" [uid="1_10"]
            - menuitem "Add work content" [uid="1_11"]
            - menuitem "Upload images and files" [uid="1_12"]
            - menuitem "Attach cloud files from OneDrive" [uid="1_13"]
        "#;
        assert_eq!(
            find_copilot_add_content_uid(snapshot),
            Some("1_10".to_string())
        );
        assert_eq!(
            find_copilot_upload_images_and_files_uid(snapshot),
            Some("1_12".to_string())
        );
    }

    #[test]
    fn finds_copilot_attachment_controls_in_traditional_chinese_snapshot() {
        let snapshot = r#"
            - button "新增內容" [uid="2_10"]
            - menuitem "新增工作內容" [uid="2_11"]
            - menuitem "上傳圖片和檔案" [uid="2_12"]
            - menuitem "附加雲端檔案" [uid="2_13"]
        "#;
        assert_eq!(
            find_copilot_add_content_uid(snapshot),
            Some("2_10".to_string())
        );
        assert_eq!(
            find_copilot_upload_images_and_files_uid(snapshot),
            Some("2_12".to_string())
        );
    }

    #[test]
    fn finds_copilot_attachment_controls_in_simplified_chinese_snapshot() {
        let snapshot = r#"
            - button "添加内容" [uid="3_10"]
            - menuitem "添加工作内容" [uid="3_11"]
            - menuitem "上传图像和文件" [uid="3_12"]
            - menuitem "附加云端文件" [uid="3_13"]
        "#;
        assert_eq!(
            find_copilot_add_content_uid(snapshot),
            Some("3_10".to_string())
        );
        assert_eq!(
            find_copilot_upload_images_and_files_uid(snapshot),
            Some("3_12".to_string())
        );
    }

    #[test]
    fn copilot_attachment_selector_uses_explicit_localized_indicators() {
        let selector = COPILOT_ATTACHMENT_INDICATOR_SELECTOR;
        for expected in [
            "attachment-chip",
            "attachment-card",
            "attachment-preview",
            "remove attachment",
            "delete attachment",
            "remove file",
            "delete file",
            "remove image",
            "delete image",
            "移除附件",
            "刪除附件",
            "删除附件",
        ] {
            assert!(
                selector.contains(expected),
                "attachment selector should include {expected:?}"
            );
        }
        assert!(
            !selector
                .split(',')
                .any(|candidate| candidate.trim().eq_ignore_ascii_case("img")),
            "a bare img selector can count unrelated composer icons"
        );
    }

    #[test]
    fn copilot_attachment_baseline_requires_zero_idle_error_free_indicators() {
        let clean = AttachmentUiOverview {
            indicator_count: 0,
            busy: false,
            error: None,
        };
        assert!(validate_copilot_attachment_baseline(&clean).is_ok());

        let stale = AttachmentUiOverview {
            indicator_count: 1,
            ..clean.clone()
        };
        assert!(validate_copilot_attachment_baseline(&stale).is_err());

        let busy = AttachmentUiOverview {
            busy: true,
            ..clean.clone()
        };
        assert!(validate_copilot_attachment_baseline(&busy).is_err());

        let errored = AttachmentUiOverview {
            error: Some("upload failed".to_string()),
            ..clean
        };
        assert!(validate_copilot_attachment_baseline(&errored).is_err());
    }

    #[test]
    fn copilot_attachment_receipt_requires_an_exact_indicator_count() {
        let overview = |indicator_count| AttachmentUiOverview {
            indicator_count,
            busy: false,
            error: None,
        };

        assert!(validate_copilot_attachment_receipt_overview(&overview(1), 2).is_err());
        assert!(validate_copilot_attachment_receipt_overview(&overview(2), 2).is_ok());
        assert!(validate_copilot_attachment_receipt_overview(&overview(3), 2).is_err());
    }

    #[test]
    fn copilot_click_send_script_injects_expected_count_and_guards_before_click() {
        let empty_receipt = AttachmentReceipt::default();
        assert_eq!(expected_copilot_attachment_count(&empty_receipt), 0);

        let receipt = AttachmentReceipt {
            paths: vec!["screen.png".to_string()],
            expected_indicator_count: 7,
            ..AttachmentReceipt::default()
        };
        assert_eq!(expected_copilot_attachment_count(&receipt), 7);

        let script = build_copilot_click_send_js(expected_copilot_attachment_count(&receipt))
            .expect("Copilot click-send script should build");
        assert!(script.contains("const expectedAttachmentCount = 7;"));
        assert!(script.contains(COPILOT_ATTACHMENT_INDICATOR_SELECTOR));
        assert!(!script.contains("__EXPECTED_ATTACHMENT_COUNT__"));
        assert!(!script.contains("__ATTACHMENT_SELECTOR__"));
        assert!(script.contains("const seen = new Set();"));
        assert!(script.contains("existing.contains(candidate)"));
        assert!(script.contains("[aria-busy=\"true\"]"));

        let state_guard = script
            .find("const attachmentState = readAttachmentState();")
            .expect("script should read attachment state after finding Send");
        let busy_guard = script[state_guard..]
            .find("if (attachmentState.busy)")
            .map(|offset| state_guard + offset)
            .expect("script should reject a busy upload");
        let count_guard = script[busy_guard..]
            .find("attachmentState.indicatorCount !== expectedAttachmentCount")
            .map(|offset| busy_guard + offset)
            .expect("script should reject an unexpected attachment count");
        let click = script[count_guard..]
            .find("button.click();")
            .map(|offset| count_guard + offset)
            .expect("script should click only after both guards");
        assert!(state_guard < busy_guard && busy_guard < count_guard && count_guard < click);
        assert!(
            !script[state_guard..click].contains("await"),
            "attachment guard and click must remain in one synchronous JavaScript turn"
        );
    }

    #[test]
    fn copilot_attachment_upload_requires_exactly_one_new_stable_indicator() {
        let before = AttachmentUiState {
            indicator_count: 1,
            file_visible: false,
            busy: false,
            error: None,
        };
        let count_increased = AttachmentUiState {
            indicator_count: 2,
            ..before.clone()
        };
        assert!(copilot_attachment_upload_ready(&before, &count_increased));

        let filename_appeared = AttachmentUiState {
            file_visible: true,
            ..before.clone()
        };
        assert!(!copilot_attachment_upload_ready(
            &before,
            &filename_appeared
        ));

        let two_indicators_appeared = AttachmentUiState {
            indicator_count: 3,
            ..before.clone()
        };
        assert!(!copilot_attachment_upload_ready(
            &before,
            &two_indicators_appeared
        ));

        let still_busy = AttachmentUiState {
            indicator_count: 2,
            busy: true,
            ..before.clone()
        };
        assert!(!copilot_attachment_upload_ready(&before, &still_busy));

        let upload_error = AttachmentUiState {
            indicator_count: 2,
            error: Some("upload failed".to_string()),
            ..before.clone()
        };
        assert!(!copilot_attachment_upload_ready(&before, &upload_error));

        assert!(!copilot_attachment_upload_ready(&before, &before));
    }

    #[test]
    fn maps_microsoft_365_copilot_attachment_mime_types() {
        assert_eq!(mime_type_for_extension("tiff"), "image/tiff");
        assert_eq!(
            mime_type_for_extension("xlsm"),
            "application/vnd.ms-excel.sheet.macroEnabled.12"
        );
        assert_eq!(mime_type_for_extension("dart"), "text/x-dart");
        assert_eq!(mime_type_for_extension("config"), "text/plain");
        assert_eq!(
            mime_type_for_extension("loop"),
            "application/vnd.microsoft.loop"
        );
    }

    #[test]
    fn rejects_gemini_image_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "gemini",
            "--image",
            "token.png",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Gemini, &cli).is_err());
    }

    #[test]
    fn allows_claude_image_and_file_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "claude",
            "--image",
            "token.png",
            "--file",
            "token.txt",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Claude, &cli).is_ok());
    }

    #[test]
    fn allows_gemini_file_attachments() {
        let cli = Cli::try_parse_from([
            "ask-bridge",
            "--provider",
            "gemini",
            "--file",
            "token.txt",
            "read",
        ])
        .unwrap();
        assert!(validate_provider_feature_support(Provider::Gemini, &cli).is_ok());
    }

    #[test]
    fn finds_linux_google_chrome_command_from_path() {
        let root = make_test_dir("chrome_path");
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();

        let stable_path = first_dir.join("google-chrome-stable");
        let chrome_path = second_dir.join("google-chrome");
        std::fs::write(&stable_path, "").unwrap();
        std::fs::write(&chrome_path, "").unwrap();

        let path_env = std::env::join_paths([first_dir.as_os_str(), second_dir.as_os_str()])
            .expect("test PATH should be joinable");

        let found = find_linux_chrome_path(Some(path_env.as_os_str()), &[]);

        assert_eq!(found, Some(chrome_path.to_string_lossy().to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finds_linux_chrome_from_standard_candidates_when_path_misses() {
        let root = make_test_dir("chrome_candidate");
        std::fs::create_dir_all(&root).unwrap();
        let chrome_path = root.join("google-chrome");
        std::fs::write(&chrome_path, "").unwrap();

        let chrome_path_str = chrome_path.to_string_lossy().to_string();
        let candidates = [chrome_path_str.as_str()];

        let found = find_linux_chrome_path(None, &candidates);

        assert_eq!(found, Some(chrome_path_str));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn returns_none_when_linux_chrome_is_missing() {
        assert_eq!(find_linux_chrome_path(None, &[]), None);
    }

    #[test]
    fn matches_profile_argument_with_quotes_and_slashes() {
        let command = r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --remote-debugging-port=9223 "--user-data-dir=C:\Users\Will\.config\ask-bridge\chrome-profile""#;
        let profile_path = r"C:/Users/Will/.config/ask-bridge/chrome-profile";

        assert!(command_uses_profile(command, profile_path));
    }

    #[test]
    fn matches_profile_argument_when_value_is_separated_by_space() {
        let command = r#"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9223 --user-data-dir /Users/will/.config/ask-bridge/chrome-profile"#;
        let profile_path = "/Users/will/.config/ask-bridge/chrome-profile";

        assert!(command_uses_profile(command, profile_path));
    }

    #[test]
    fn rejects_different_profile_argument() {
        let command = r#"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9223 --user-data-dir=/Users/will/.config/other/chrome-profile"#;
        let profile_path = "/Users/will/.config/ask-bridge/chrome-profile";

        assert!(!command_uses_profile(command, profile_path));
    }

    #[test]
    fn rejects_profile_and_marker_prefixes_with_extra_suffixes() {
        let profile_path = r"C:\Users\Will\.config\ask-bridge\chrome-profile";
        let profile_copy =
            r#"chrome.exe --user-data-dir=C:\Users\Will\.config\ask-bridge\chrome-profile-copy"#;
        let marker_copy = "chrome.exe --ask-bridge-instance-copy";

        assert!(!command_uses_profile(profile_copy, profile_path));
        assert!(!command_identifies_ask_chrome(marker_copy, profile_path));
    }

    #[test]
    fn composer_without_account_or_auth_controls_has_logged_in_state() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedIn);
    }

    #[test]
    fn chatgpt_login_signals_wait_for_ambiguous_auth_shell() {
        let script = Provider::ChatGpt.login_signals_js();

        assert!(script.starts_with("async () =>"));
        assert!(script.contains("earliestDecision"));
        assert!(script.contains("stableSince"));
        assert!(script.contains("let stable = false"));
        assert!(script.contains("JSON.stringify(nextSignals)"));
        assert!(script.contains("await new Promise"));
        assert!(script.contains("Date.now() + 5000"));
        assert!(script.contains("return { ...signals, stable }"));
    }

    #[test]
    fn account_control_has_logged_in_state() {
        let signals = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedIn);
    }

    #[test]
    fn auth_control_or_auth_path_has_logged_out_state() {
        let visible_auth_control = LoginSignals {
            account: false,
            auth_control: true,
            auth_path: false,
            composer: true,
            stable: true,
        };
        let auth_path = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: true,
            composer: false,
            stable: false,
        };

        assert_eq!(
            visible_auth_control.state(Provider::ChatGpt),
            LoginState::LoggedOut
        );
        assert_eq!(auth_path.state(Provider::ChatGpt), LoginState::LoggedOut);
    }

    #[test]
    fn direct_query_opens_login_and_continues_after_success() {
        let mut opened_login = false;
        let mut waited_with_timeout = None;

        complete_query_login_with(
            Provider::Copilot,
            37,
            || {
                opened_login = true;
                Ok(())
            },
            |timeout| {
                waited_with_timeout = Some(timeout);
                (LoginState::LoggedIn, false)
            },
        )
        .expect("successful automatic login should continue the original query");

        assert!(opened_login);
        assert_eq!(waited_with_timeout, Some(37));
    }

    #[test]
    fn direct_query_login_timeout_stops_before_sending_the_prompt() {
        let mut opened_login = false;
        let error = complete_query_login_with(
            Provider::Copilot,
            12,
            || {
                opened_login = true;
                Ok(())
            },
            |_| (LoginState::LoggedOut, true),
        )
        .expect_err("an incomplete login must not send the original query");

        assert!(opened_login);
        assert!(error.contains("Timed out after 12 seconds"));
        assert!(error.contains("Microsoft 365 Copilot"));
    }

    #[test]
    fn direct_query_does_not_wait_when_login_window_cannot_be_shown() {
        let mut waited_for_login = false;
        let error = complete_query_login_with(
            Provider::Copilot,
            300,
            || Err("could not restore Chrome".to_string()),
            |_| {
                waited_for_login = true;
                (LoginState::LoggedIn, false)
            },
        )
        .expect_err("window restore failures must be reported immediately");

        assert_eq!(error, "could not restore Chrome");
        assert!(!waited_for_login);
    }

    #[test]
    fn empty_login_signals_have_unknown_state() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: false,
            stable: true,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::Unknown);
    }

    #[test]
    fn unstable_chatgpt_signals_never_block_or_confirm_login() {
        for signals in [
            LoginSignals {
                account: false,
                auth_control: true,
                auth_path: false,
                composer: true,
                stable: false,
            },
            LoginSignals {
                account: false,
                auth_control: false,
                auth_path: false,
                composer: true,
                stable: false,
            },
        ] {
            assert_eq!(signals.state(Provider::ChatGpt), LoginState::Unknown);
        }
    }

    #[test]
    fn auth_path_overrides_stale_account_control() {
        let signals = LoginSignals {
            account: true,
            auth_control: false,
            auth_path: true,
            composer: true,
            stable: false,
        };

        assert_eq!(signals.state(Provider::ChatGpt), LoginState::LoggedOut);
    }

    #[test]
    fn gemini_composer_without_account_remains_unknown() {
        let signals = LoginSignals {
            account: false,
            auth_control: false,
            auth_path: false,
            composer: true,
            stable: true,
        };

        assert_eq!(signals.state(Provider::Gemini), LoginState::Unknown);
    }

    #[test]
    fn prefers_logged_in_provider_page_over_selected_page() {
        let pages = [
            PageLoginState {
                id: 2,
                selected: true,
                login_state: LoginState::LoggedOut,
            },
            PageLoginState {
                id: 7,
                selected: false,
                login_state: LoginState::LoggedIn,
            },
        ];

        assert_eq!(preferred_provider_page_id(&pages), Some(7));
    }

    #[test]
    fn falls_back_to_selected_provider_page_when_none_are_logged_in() {
        let pages = [
            PageLoginState {
                id: 2,
                selected: false,
                login_state: LoginState::Unknown,
            },
            PageLoginState {
                id: 7,
                selected: true,
                login_state: LoginState::LoggedOut,
            },
        ];

        assert_eq!(preferred_provider_page_id(&pages), Some(7));
    }

    #[test]
    fn chrome_launch_args_put_headful_windows_on_the_visible_desktop() {
        assert_eq!(
            chrome_window_launch_args(false),
            [CHROME_WINDOW_SIZE_ARG, "--window-position=80,80"]
        );
        assert!(
            !chrome_window_launch_args(false)
                .iter()
                .any(|argument| argument.contains("background") || argument.contains("-2000"))
        );
    }

    #[test]
    fn chrome_launch_args_preserve_the_background_window_position() {
        assert_eq!(
            chrome_window_launch_args(true),
            [
                "--ask-bridge-background",
                "--disable-blink-features=AutomationControlled",
                CHROME_WINDOW_SIZE_ARG,
                "--window-position=-2000,-2000",
            ]
        );
    }

    #[test]
    fn headful_reuse_moves_the_identified_chrome_window_onscreen() {
        let snapshot = ChromeDebugSnapshot {
            listener_pids: vec!["20728".to_string()],
            record: Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            }),
            browser_id: Some("browser-123".to_string()),
            ask_pids: vec!["30000".to_string()],
        };
        let mut calls = Vec::new();

        apply_chrome_window_mode_with(false, &snapshot, |pids| {
            calls.push(pids.to_vec());
            Ok(())
        })
        .expect("headful reuse should restore the existing Chrome window");

        assert_eq!(calls, vec![vec![20728, 30000]]);
    }

    #[test]
    fn chrome_window_candidates_never_trust_a_raw_or_mismatched_listener() {
        let fake_listener_snapshot = ChromeDebugSnapshot {
            listener_pids: vec!["666".to_string()],
            record: Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            }),
            browser_id: Some("browser-123".to_string()),
            ask_pids: vec!["30000".to_string()],
        };
        assert_eq!(
            chrome_window_candidate_pids(&fake_listener_snapshot),
            vec![30000]
        );

        let raw_listener_only = ChromeDebugSnapshot {
            listener_pids: vec!["666".to_string()],
            record: None,
            browser_id: Some("browser-123".to_string()),
            ask_pids: Vec::new(),
        };
        assert!(chrome_window_candidate_pids(&raw_listener_only).is_empty());
    }

    #[test]
    fn background_reuse_never_moves_the_managed_chrome_window() {
        let snapshot = ChromeDebugSnapshot {
            listener_pids: vec!["20728".to_string()],
            record: Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            }),
            browser_id: Some("browser-123".to_string()),
            ask_pids: vec!["20728".to_string()],
        };

        apply_chrome_window_mode_with(true, &snapshot, |_| {
            panic!("background reuse must not move Chrome onto the visible desktop")
        })
        .expect("background reuse should preserve its offscreen window mode");
    }

    #[test]
    fn headful_reuse_propagates_window_restore_failures() {
        let snapshot = ChromeDebugSnapshot {
            listener_pids: vec!["20728".to_string()],
            record: Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            }),
            browser_id: Some("browser-123".to_string()),
            ask_pids: vec![],
        };

        let error = apply_chrome_window_mode_with(false, &snapshot, |pids| {
            assert_eq!(pids, [20728]);
            Err("window move failed".to_string())
        })
        .expect_err("headful mode must not silently reuse an offscreen Chrome window");

        assert_eq!(error, "window move failed");
    }

    #[test]
    fn chrome_process_image_path_requires_a_full_case_insensitive_match() {
        assert!(chrome_image_paths_match(
            r"C:\Program Files\Google\Chrome\Application\CHROME.EXE",
            r"c:\program files\google\chrome\application\chrome.exe"
        ));
        assert!(!chrome_image_paths_match(
            r"C:\Temp\chrome.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        ));
        assert!(!chrome_image_paths_match(
            r"chrome.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        ));
    }

    #[test]
    fn chrome_window_predicate_requires_pid_class_owner_and_title_checks() {
        assert!(chrome_window_matches_predicate(
            true,
            "Chrome_WidgetWin_1",
            false,
            12
        ));
        assert!(!chrome_window_matches_predicate(
            false,
            "Chrome_WidgetWin_1",
            false,
            12
        ));
        assert!(!chrome_window_matches_predicate(
            true,
            "Chrome_RenderWidgetHostHWND",
            false,
            12
        ));
        assert!(!chrome_window_matches_predicate(
            true,
            "Chrome_WidgetWin_1",
            true,
            12
        ));
        assert!(!chrome_window_matches_predicate(
            true,
            "Chrome_WidgetWin_1",
            false,
            0
        ));
    }

    #[test]
    fn screen_rect_intersection_requires_positive_visible_area() {
        let monitor = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert!(screen_rects_intersect(
            ScreenRect {
                left: 80,
                top: 80,
                right: 1000,
                bottom: 900,
            },
            monitor
        ));
        assert!(screen_rects_intersect(
            ScreenRect {
                left: -100,
                top: 50,
                right: 100,
                bottom: 200,
            },
            monitor
        ));
        assert!(!screen_rects_intersect(
            ScreenRect {
                left: -500,
                top: 50,
                right: 0,
                bottom: 200,
            },
            monitor
        ));
        assert!(!screen_rects_intersect(
            ScreenRect {
                left: 80,
                top: 80,
                right: 80,
                bottom: 200,
            },
            monitor
        ));
    }

    #[test]
    fn chrome_window_visibility_requires_a_meaningful_monitor_area() {
        let monitor = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };

        assert!(screen_rect_visible_area_at_least(
            ScreenRect {
                left: 80,
                top: 80,
                right: 1280,
                bottom: 980,
            },
            monitor,
            CHROME_MIN_VISIBLE_WIDTH,
            CHROME_MIN_VISIBLE_HEIGHT,
        ));
        assert!(!screen_rect_visible_area_at_least(
            ScreenRect {
                left: 80,
                top: 80,
                right: 81,
                bottom: 81,
            },
            monitor,
            CHROME_MIN_VISIBLE_WIDTH,
            CHROME_MIN_VISIBLE_HEIGHT,
        ));
        assert!(!screen_rect_visible_area_at_least(
            ScreenRect {
                left: -1000,
                top: 80,
                right: -700,
                bottom: 500,
            },
            monitor,
            CHROME_MIN_VISIBLE_WIDTH,
            CHROME_MIN_VISIBLE_HEIGHT,
        ));
        assert!(!screen_rect_visible_area_at_least(
            ScreenRect {
                left: -100,
                top: 80,
                right: 200,
                bottom: 500,
            },
            monitor,
            CHROME_MIN_VISIBLE_WIDTH,
            CHROME_MIN_VISIBLE_HEIGHT,
        ));
    }

    #[test]
    fn marker_identifies_ask_bridge_chrome_without_profile_argument() {
        let command = r#"chrome.exe --type=browser --ask-bridge-instance"#;

        assert!(command_identifies_ask_chrome(
            command,
            r"C:\Users\Will\.config\ask-bridge\chrome-profile"
        ));
    }

    #[test]
    fn parses_legacy_and_json_chrome_process_records() {
        assert_eq!(
            parse_chrome_process_record("15864\r\n"),
            Some(ChromeProcessRecord {
                pid: 15864,
                browser_id: None,
            })
        );
        assert_eq!(
            parse_chrome_process_record(r#"{"pid":20728,"browser_id":"browser-123"}"#),
            Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            })
        );
    }

    #[test]
    fn extracts_browser_id_from_cdp_version_response() {
        let body = r#"{"Browser":"Chrome/149","webSocketDebuggerUrl":"ws://127.0.0.1:9223/devtools/browser/browser-123"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\nContent-Type:application/json\r\n\r\n{}",
            body.len(),
            body
        );

        assert_eq!(
            browser_id_from_version_response(&response),
            Some("browser-123".to_string())
        );
        assert!(http_response_is_complete(response.as_bytes()));
        assert!(!http_response_is_complete(
            &response.as_bytes()[..response.len() - 1]
        ));

        let non_success = response.replacen("200 OK", "404 Not Found", 1);
        assert_eq!(browser_id_from_version_response(&non_success), None);
        assert_eq!(browser_id_from_version_response(body), None);

        let foreign_body = body.replace("127.0.0.1:9223", "example.com:9223");
        let foreign_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\n\r\n{}",
            foreign_body.len(),
            foreign_body
        );
        assert_eq!(browser_id_from_version_response(&foreign_response), None);

        let overflowing_length = format!(
            "HTTP/1.1 200 OK\r\nContent-Length:{}\r\n\r\n{{}}",
            usize::MAX
        );
        assert!(!http_response_is_complete(overflowing_length.as_bytes()));
    }

    #[test]
    fn build_chrome_process_record_prefers_unique_listener_pid() {
        let listeners = vec!["20728".to_string()];
        assert_eq!(
            build_chrome_process_record(&listeners, Some("browser-123")),
            Some(ChromeProcessRecord {
                pid: 20728,
                browser_id: Some("browser-123".to_string()),
            })
        );
    }

    #[test]
    fn build_chrome_process_record_requires_unambiguous_identity() {
        assert_eq!(
            build_chrome_process_record(
                &["20728".to_string(), "30000".to_string()],
                Some("browser-123")
            ),
            None
        );
        assert_eq!(
            build_chrome_process_record(&["20728".to_string()], None),
            None
        );
    }

    #[test]
    fn chrome_record_matches_current_checks_browser_identity_and_scope() {
        let record = ChromeProcessRecord {
            pid: 20728,
            browser_id: Some("browser-123".to_string()),
        };
        let single = vec!["20728".to_string()];
        let multiple = vec!["20728".to_string(), "30000".to_string()];

        assert!(chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &single
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-456"),
            &single
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &multiple
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &["30000".to_string()]
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &["not-a-pid".to_string()]
        ));
        assert!(!chrome_record_matches_current(
            Some(&record),
            Some("browser-123"),
            &["020728".to_string()]
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_netstat_parser_matches_exact_listening_port() {
        let output = concat!(
            "  TCP    127.0.0.1:9223    0.0.0.0:0    LISTENING    20728\r\n",
            "  TCP    127.0.0.1:92230   0.0.0.0:0    LISTENING    30000\r\n",
            "  TCP    [::1]:9223        [::]:0       LISTENING    20728\r\n",
            "  TCP    127.0.0.1:9223    127.0.0.1:50000 ESTABLISHED 40000\r\n",
            "  UDP    127.0.0.1:9223    *:*                       50000\r\n"
        );

        assert_eq!(
            parse_windows_netstat_listener_pids(output, 9223),
            vec!["20728".to_string()]
        );
    }

    #[test]
    fn finds_ask_owner_pids_and_deduplicates_results() {
        let listeners = vec![
            "30000".to_string(),
            "20728".to_string(),
            "20728".to_string(),
        ];
        let commands = std::collections::HashMap::from([
            ("20728", "chrome.exe --type=utility"),
            ("30000", "chrome.exe --type=gpu-process"),
            (
                "18000",
                "chrome.exe --remote-debugging-port=9223 --ask-bridge-instance",
            ),
            (
                "15000",
                "chrome.exe --user-data-dir=C:\\Users\\Chris\\.config\\ask-bridge\\chrome-profile",
            ),
        ]);
        let parents = std::collections::HashMap::from([
            ("20728", "18000"),
            ("30000", "18000"),
            ("18000", "1"),
            ("15000", "1"),
        ]);

        let ask_pids = find_ask_chrome_owner_pids_with(
            &listeners,
            r"C:\Users\Chris\.config\ask-bridge\chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(ask_pids, vec!["18000".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_wmic_value_after_blank_lines() {
        let output = "CommandLine\r\n\r\n  chrome.exe --remote-debugging-port=9223  \r\n\r\n";

        assert_eq!(
            parse_wmic_column_value(output),
            Some("chrome.exe --remote-debugging-port=9223".to_string())
        );
    }

    #[test]
    fn finds_ask_chrome_owner_in_parent_process_chain() {
        let commands = std::collections::HashMap::from([
            ("100", "chrome.exe --type=utility"),
            (
                "50",
                "chrome.exe --remote-debugging-port=9223 --ask-bridge-instance",
            ),
        ]);
        let parents = std::collections::HashMap::from([("100", "50"), ("50", "1")]);

        let owner = find_ask_chrome_owner_pid_with(
            "100",
            "/tmp/ask-bridge/chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(owner, Some("50".to_string()));
    }

    #[test]
    fn rejects_process_chain_without_profile_or_marker() {
        let commands = std::collections::HashMap::from([
            ("100", "chrome.exe --type=utility"),
            ("50", "chrome.exe --remote-debugging-port=9223"),
        ]);
        let parents = std::collections::HashMap::from([("100", "50"), ("50", "1")]);

        let owner = find_ask_chrome_owner_pid_with(
            "100",
            "/tmp/ask-bridge/chrome-profile",
            |pid| commands.get(pid).map(|command| (*command).to_string()),
            |pid| parents.get(pid).map(|parent| (*parent).to_string()),
        );

        assert_eq!(owner, None);
    }
}

fn read_clipboard() -> Result<String, String> {
    let output = Command::new("pbpaste")
        .output()
        .map_err(|e| format!("Failed to run pbpaste: {}", e))?;

    if !output.status.success() {
        return Err(format!("pbpaste exited with status: {}", output.status));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn write_clipboard(content: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run pbcopy: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| format!("Failed to write clipboard content: {}", e))?;
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for pbcopy: {}", e))?;

    if !status.success() {
        return Err(format!("pbcopy exited with status: {}", status));
    }

    Ok(())
}

fn click_latest_copy_button(config_path: &str, provider: Provider) -> Result<(), String> {
    let response_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let script = r#"() => {
                const isVisible = (el) => {
                    if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                    const style = window.getComputedStyle(el);
                    if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                    const rect = el.getBoundingClientRect();
                    return rect.width > 0 && rect.height > 0;
                };

                const labelOf = (el) => [
                    el.getAttribute('aria-label'),
                    el.getAttribute('title'),
                    el.getAttribute('data-testid'),
                    el.textContent
                ].filter(Boolean).join(' ');

                const isCopyButton = (el) => {
                    const label = labelOf(el);
                    return /copy|複製|复制|コピー|복사/i.test(label)
                        && !/prompt|提示詞|提示词|入力|table|表格/i.test(label);
                };
                const copyButtonScore = (el) => {
                    const label = labelOf(el);
                    if (!isCopyButton(el) || !isVisible(el)) return -1;
                    if (el.closest('pre, code, [class*="code"], [data-testid*="code"]')) return -1;
                    if (/copy-turn-action-button/i.test(label)) return 100;
                    if (/response|回應|回答|reply/i.test(label)) return 90;
                    if (el.closest('model-response, response-container, [data-message-author-role="assistant"], .agent-turn, [data-is-streaming], .font-claude-response')) return 50;
                    return 10;
                };
                const messages = Array.from(document.querySelectorAll(__RESPONSE_SELECTOR__));
                const latest = messages[messages.length - 1];
                if (!latest) return { ok: false, reason: "No assistant message found" };

                latest.scrollIntoView({ block: 'center', inline: 'nearest' });
                for (const type of ['pointerover', 'mouseover', 'mouseenter']) {
                    latest.dispatchEvent(new MouseEvent(type, { bubbles: true, view: window }));
                }

                const scopes = [
                    latest,
                    latest.closest('article'),
                    latest.closest('[data-testid^="conversation-turn"]'),
                    latest.parentElement,
                    latest.parentElement?.parentElement
                ].filter(Boolean);

                for (const scope of scopes) {
                    const buttons = Array.from(scope.querySelectorAll('button'));
                    const candidates = buttons
                        .map((button) => ({ button, score: copyButtonScore(button) }))
                        .filter((candidate) => candidate.score >= 0)
                        .sort((a, b) => b.score - a.score);
                    if (candidates.length > 0) {
                        const button = candidates[0].button;
                        button.click();
                        return { ok: true, label: labelOf(button) };
                    }
                }

                return { ok: false, reason: "Copy response button not found" };
            }"#
    .replace("__RESPONSE_SELECTOR__", &response_selector);
    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": script
        }),
    )?;

    let parsed = parse_script_result(&res)?;
    if parsed["ok"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(parsed["reason"]
            .as_str()
            .unwrap_or("Failed to click copy response button")
            .to_string())
    }
}

fn wait_for_page_load(config_path: &str, provider: Provider, verbose: bool) -> Result<(), String> {
    if verbose {
        println!("Waiting for page readyState...");
    }

    // Phase 1: Wait for readyState complete or interactive
    let mut ready = false;
    for _ in 0..90 {
        let ready_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => document.readyState === 'complete' || document.readyState === 'interactive'"
            }),
        );

        if ready_res
            .and_then(|res| parse_script_result(&res))
            .map(|parsed| parsed.as_bool().unwrap_or(false))
            .unwrap_or(false)
        {
            ready = true;
            break;
        }

        thread::sleep(Duration::from_millis(500));
    }

    if !ready {
        return Err("Timeout waiting for page readyState to be loaded".to_string());
    }

    if verbose {
        println!("Waiting for {} page elements...", provider.display_name());
    }

    // Phase 2: Wait for key provider elements to render.
    for _ in 0..60 {
        let element_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": provider.ready_check_js()
            }),
        );

        if element_res
            .and_then(|res| parse_script_result(&res))
            .map(|parsed| parsed.as_bool().unwrap_or(false))
            .unwrap_or(false)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }

    if verbose {
        println!(
            "Warning: Timeout waiting for {} page elements. Proceeding anyway...",
            provider.display_name()
        );
    }
    Ok(())
}

fn open_url_tab(
    config_path: &str,
    provider: Provider,
    url: &str,
    headless: bool,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        println!("Opening URL: {}", url);
    }

    let list_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
    let text = list_res
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("Invalid list_pages response structure: {:?}", list_res))?;

    let pages = parse_pages(text);
    if pages.len() == 1
        && (pages[0].url == "about:blank"
            || pages[0].url.contains("new-tab-page")
            || pages[0].url.contains("chrome://welcome"))
    {
        call_mcp_tool(
            config_path,
            "navigate_page",
            serde_json::json!({
                "url": url
            }),
        )?;
    } else {
        call_mcp_tool(
            config_path,
            "new_page",
            serde_json::json!({
                "url": url
            }),
        )?;
    }

    for _ in 0..20 {
        let refreshed_pages_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
        let refreshed_text = refreshed_pages_res
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                format!(
                    "Invalid refreshed list_pages response structure: {:?}",
                    refreshed_pages_res
                )
            })?;

        let refreshed_pages = parse_pages(refreshed_text);
        if let Some(page) = refreshed_pages.iter().find(|page| page.url == url) {
            call_mcp_tool(
                config_path,
                "select_page",
                serde_json::json!({
                    "pageId": page.id,
                    "bringToFront": !headless
                }),
            )?;

            for stale_page in refreshed_pages.iter().filter(|p| p.id != page.id) {
                let _ = call_mcp_tool(
                    config_path,
                    "close_page",
                    serde_json::json!({
                        "pageId": stale_page.id
                    }),
                );
            }

            let page_provider = Provider::from_url(url).unwrap_or(provider);
            return wait_for_page_load(config_path, page_provider, verbose);
        }

        thread::sleep(Duration::from_millis(250));
    }

    let page_provider = Provider::from_url(url).unwrap_or(provider);
    wait_for_page_load(config_path, page_provider, verbose)
}

fn copy_latest_markdown(config_path: &str, provider: Provider) -> Result<String, String> {
    match copy_latest_markdown_via_clipboard(config_path, provider) {
        Ok(content) => Ok(content),
        Err(_) => scrape_latest_markdown_from_dom(config_path, provider),
    }
}

fn copy_latest_markdown_via_clipboard(
    config_path: &str,
    provider: Provider,
) -> Result<String, String> {
    let clipboard_before = read_clipboard().unwrap_or_default();
    let sentinel = format!("__ASK_CHATGPT_COPY_PENDING_{}__", std::process::id());
    write_clipboard(&sentinel)?;

    // Click the copy button, retrying if the message or button is not found yet (due to asynchronous rendering of Single Page App)
    let mut click_err = None;
    for _ in 0..30 {
        match click_latest_copy_button(config_path, provider) {
            Ok(_) => {
                click_err = None;
                break;
            }
            Err(e) => {
                click_err = Some(e);
                thread::sleep(Duration::from_millis(500));
            }
        }
    }

    if let Some(err) = click_err {
        // Restore clipboard before returning error
        let _ = write_clipboard(&clipboard_before);
        return Err(format!("Error copying latest response Markdown: {}", err));
    }

    let mut copied_content = None;
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        match read_clipboard() {
            Ok(content) if !content.trim().is_empty() && content != sentinel => {
                copied_content = Some(content);
                break;
            }
            _ => {}
        }
    }

    // Always restore the original clipboard
    let _ = write_clipboard(&clipboard_before);

    let content = copied_content
        .ok_or_else(|| "Timed out waiting for clipboard content after clicking copy".to_string())?;

    // Create a temporary file path
    let temp_path = std::env::temp_dir().join(format!("ask_chatgpt_{}.md", std::process::id()));

    // Write the copied content immediately to the temporary file
    std::fs::write(&temp_path, &content)
        .map_err(|e| format!("Failed to write to temporary file: {}", e))?;

    // Read the content back from the temporary file to output to the terminal
    let verified_content = std::fs::read_to_string(&temp_path)
        .map_err(|e| format!("Failed to read from temporary file: {}", e))?;

    // Clean up temporary file
    let _ = std::fs::remove_file(&temp_path);

    Ok(verified_content)
}

fn scrape_latest_markdown_from_dom(
    config_path: &str,
    provider: Provider,
) -> Result<String, String> {
    let latest_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let content_selector = serde_json::to_string(provider.response_content_selector())
        .map_err(|e| format!("Failed to serialize response content selector: {}", e))?;
    let inspect_js = r#"() => {
        const latestSelector = __LATEST_SELECTOR__;
        const contentSelector = __CONTENT_SELECTOR__;
        const isActionToolbar = (el) => {
            if (!el) return false;
            const classText = Array.from(el.classList || []).join(' ');
            return el.getAttribute('role') === 'toolbar' ||
                /(?:^|\s)fai-CopilotMessage__actions(?:\s|$)/.test(classText);
        };
        const messages = Array.from(document.querySelectorAll(latestSelector))
            .filter((el) => !isActionToolbar(el) &&
                ((el.innerText || el.textContent || '').trim().length > 0));
        let latest = messages[messages.length - 1];
        if (!latest) {
            const labelOf = (el) => [
                el.getAttribute('aria-label'),
                el.getAttribute('title'),
                el.getAttribute('data-testid'),
                el.textContent
            ].filter(Boolean).join(' ');
            const copyButtons = Array.from(document.querySelectorAll('button'))
                .filter((button) => {
                    const label = labelOf(button);
                    return /copy|複製|复制|コピー|복사/i.test(label) &&
                        !/code|程式碼|代码|table|表格/i.test(label) &&
                        !button.closest('pre, code, [class*=\"code\"], [data-testid*=\"code\"]');
                });

            for (const button of copyButtons.reverse()) {
                let candidate = button.parentElement;
                while (candidate && candidate !== document.body && candidate !== document.documentElement) {
                    const clone = candidate.cloneNode(true);
                    clone.querySelectorAll('button, style, script, svg').forEach((el) => el.remove());
                    const text = (clone.innerText || clone.textContent || '').trim();
                    if (!isActionToolbar(candidate) && text.length > 1) {
                        latest = candidate;
                        break;
                    }
                    candidate = candidate.parentElement;
                }
                if (latest) break;
            }
        }
        if (!latest) return 'No assistant message found';
        const turn = contentSelector ? (latest.querySelector(contentSelector) || latest) : latest;
        
        const elementToMarkdown = (element) => {
            let markdown = '';
            const processedSrcs = new Set();
            const walk = (node) => {
                if (node.nodeType === Node.TEXT_NODE) {
                    markdown += node.textContent;
                    return;
                }
                if (node.nodeType !== Node.ELEMENT_NODE) return;

                const tag = node.tagName.toLowerCase();

                const classText = Array.from(node.classList || []).join(' ');
                const role = node.getAttribute('role');
                if (node.classList.contains('sr-only') ||
                    /screen-reader|visually-hidden|cdk-visually-hidden/.test(classText) ||
                    role === 'toolbar' ||
                    /(?:^|\s)fai-CopilotMessage__actions(?:\s|$)/.test(classText) ||
                    tag === 'button' || tag === 'style' || tag === 'script') {
                    return;
                }

                // Code blocks
                if (tag === 'pre') {
                    const codeEl = node.querySelector('code');
                    const langClass = codeEl ? Array.from(codeEl.classList).find(c => c.startsWith('language-')) : '';
                    const lang = langClass ? langClass.replace('language-', '') : '';
                    const codeText = codeEl ? codeEl.textContent : node.textContent;
                    markdown += '\n```' + lang + '\n' + codeText + '\n```\n';
                    return;
                }

                // Inline code
                if (tag === 'code') {
                    if (!node.closest('pre')) {
                        markdown += '`' + node.textContent + '`';
                        return;
                    }
                }

                // Bold
                if (tag === 'strong' || tag === 'b') {
                    markdown += '**';
                    for (const child of node.childNodes) walk(child);
                    markdown += '**';
                    return;
                }

                // Italics
                if (tag === 'em' || tag === 'i') {
                    markdown += '*';
                    for (const child of node.childNodes) walk(child);
                    markdown += '*';
                    return;
                }

                // Links
                if (tag === 'a') {
                    const href = node.getAttribute('href') || '';
                    const text = node.textContent || '';
                    if (href && text) {
                        markdown += '[' + text + '](' + href + ')';
                        return;
                    }
                }

                // Paragraphs, headers, list items
                if (tag === 'p') markdown += '\n';
                if (tag === 'br') markdown += '\n';
                if (tag === 'h1') markdown += '\n# ';
                if (tag === 'h2') markdown += '\n## ';
                if (tag === 'h3') markdown += '\n### ';
                if (tag === 'h4') markdown += '\n#### ';
                if (tag === 'h5') markdown += '\n##### ';
                if (tag === 'h6') markdown += '\n###### ';
                if (tag === 'li') markdown += '\n* ';

                // Images
                if (tag === 'img') {
                    const src = node.getAttribute('src') || '';
                    const alt = node.getAttribute('alt') || 'image';
                    if (src && !src.includes('avatar') && !src.includes('profile')) {
                        if (processedSrcs.has(src)) return;
                        processedSrcs.add(src);
                        markdown += '\n![' + alt + '](' + src + ')\n';
                        return;
                    }
                }

                for (const child of node.childNodes) {
                    walk(child);
                }

                if (['p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'li'].includes(tag)) {
                    markdown += '\n';
                }
            };

            walk(element);
            return markdown.trim().replace(/\n{3,}/g, '\n\n');
        };
        
        return elementToMarkdown(turn);
    }"#
    .replace("__LATEST_SELECTOR__", &latest_selector)
    .replace("__CONTENT_SELECTOR__", &content_selector);

    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": inspect_js
        }),
    )?;

    let val = parse_script_result(&res)?;
    let content = val
        .as_str()
        .ok_or_else(|| "DOM scraper returned non-string result".to_string())?
        .to_string();

    if content == "No assistant message found" {
        return Err(format!(
            "No assistant message found on {} page",
            provider.display_name()
        ));
    }

    Ok(content)
}

fn download_images_from_latest_message(
    config_path: &str,
    provider: Provider,
    image_output: Option<&str>,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        println!("Checking for generated images in the latest assistant response...");
    }
    let latest_selector = serde_json::to_string(provider.latest_response_selector())
        .map_err(|e| format!("Failed to serialize response selector: {}", e))?;
    let image_scan_js = r#"() => {
                window.__downloaded_images_status = "pending";
                window.__downloaded_images = null;
                (async () => {
                    try {
                        const messages = document.querySelectorAll(__LATEST_SELECTOR__);
                        const latestMessage = messages[messages.length - 1];
                        if (!latestMessage) {
                            window.__downloaded_images = [];
                            window.__downloaded_images_status = "success";
                            return;
                        }
                        
                        const imgs = Array.from(latestMessage.querySelectorAll('img'));
                        const seenSrcs = new Set();
                        const candidateImgs = imgs.filter(img => {
                            const src = img.src || '';
                            if (src.includes('avatar') || src.includes('profile')) return false;
                            const width = img.naturalWidth || img.width || 0;
                            const height = img.naturalHeight || img.height || 0;
                            if (width > 0 && width < 100) return false;
                            if (height > 0 && height < 100) return false;
                            if (!src.startsWith('http') && !src.startsWith('blob:') && !src.startsWith('data:image/')) return false;
                            if (seenSrcs.has(src)) return false;
                            seenSrcs.add(src);
                            return true;
                        });

                        const imagesData = [];
                        for (let i = 0; i < candidateImgs.length; i++) {
                            const img = candidateImgs[i];
                            try {
                                if (!img.complete) {
                                    await new Promise((resolve) => {
                                        img.addEventListener('load', resolve);
                                        img.addEventListener('error', resolve);
                                        setTimeout(resolve, 10000);
                                    });
                                }

                                let dataUrl = "";
                                if ((img.src || '').startsWith('data:image/')) {
                                    dataUrl = img.src;
                                } else {
                                    try {
                                        const response = await fetch(img.src);
                                        const blob = await response.blob();
                                        dataUrl = await new Promise((resolve, reject) => {
                                            const reader = new FileReader();
                                            reader.onloadend = () => resolve(reader.result);
                                            reader.onerror = reject;
                                            reader.readAsDataURL(blob);
                                        });
                                    } catch (fetchErr) {
                                        const canvas = document.createElement('canvas');
                                        canvas.width = img.naturalWidth || img.width || 512;
                                        canvas.height = img.naturalHeight || img.height || 512;
                                        const ctx = canvas.getContext('2d');
                                        ctx.drawImage(img, 0, 0);
                                        dataUrl = canvas.toDataURL('image/png');
                                    }
                                }

                                if (dataUrl && dataUrl.startsWith('data:image/')) {
                                    imagesData.push({
                                        index: i,
                                        src: img.src,
                                        alt: img.alt || "",
                                        dataUrl: dataUrl
                                    });
                                }
                            } catch (err) {
                                // ignore
                            }
                        }
                        window.__downloaded_images = imagesData;
                        window.__downloaded_images_status = "success";
                    } catch (e) {
                        window.__downloaded_images_status = "error: " + e.message;
                    }
                })();
                return { ok: true };
            }"#
    .replace("__LATEST_SELECTOR__", &latest_selector);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": image_scan_js
        }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed["ok"].as_bool().unwrap_or(false) {
        return Err("Failed to initiate image scanning script".to_string());
    }

    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 150 {
        thread::sleep(Duration::from_millis(100));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__downloaded_images_status || 'pending'"
            }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|str_ref| str_ref.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Image scanning failed: {}", status));
    }

    if status == "pending" {
        return Err("Timed out waiting for images to download in browser".to_string());
    }

    let get_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": r#"() => {
                const res = window.__downloaded_images || [];
                delete window.__downloaded_images;
                delete window.__downloaded_images_status;
                return res;
            }"#
        }),
    )?;

    let parsed = parse_script_result(&get_res)?;
    let images = match parsed.as_array() {
        Some(arr) => arr,
        None => return Ok(()),
    };

    if images.is_empty() {
        if verbose {
            println!("No generated images found in the latest response.");
        }
        return Ok(());
    }

    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let total = images.len();
    for (idx, img) in images.iter().enumerate() {
        let data_url = match img["dataUrl"].as_str() {
            Some(s) => s,
            None => continue,
        };

        let parts: Vec<&str> = data_url.splitn(2, ',').collect();
        if parts.len() != 2 {
            continue;
        }

        let header = parts[0];
        let base64_data = parts[1];

        let ext = if header.contains("image/png") {
            "png"
        } else if header.contains("image/jpeg") || header.contains("image/jpg") {
            "jpg"
        } else if header.contains("image/webp") {
            "webp"
        } else {
            "png"
        };

        let decoded = general_purpose::STANDARD
            .decode(base64_data)
            .map_err(|e| format!("Failed to decode base64 data: {}", e))?;

        let file_path = match image_output {
            Some(output_str) => {
                let path = std::path::Path::new(output_str);
                let is_dir = path.is_dir()
                    || output_str.ends_with('/')
                    || output_str.ends_with('\\')
                    || path.extension().is_none();

                if is_dir {
                    std::fs::create_dir_all(path)
                        .map_err(|e| format!("Failed to create directory {:?}: {}", path, e))?;
                    path.join(format!("generated_{}_{}.{}", epoch, idx, ext))
                } else {
                    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            format!("Failed to create parent directory {:?}: {}", parent, e)
                        })?;
                    }
                    let file_stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or_else(|| "Invalid file name".to_string())?;
                    let file_ext = path.extension().and_then(|e| e.to_str()).unwrap_or(ext);

                    if total <= 1 {
                        parent.join(format!("{}.{}", file_stem, file_ext))
                    } else {
                        parent.join(format!("{}_{}.{}", file_stem, idx + 1, file_ext))
                    }
                }
            }
            None => {
                std::fs::create_dir_all("target")
                    .map_err(|e| format!("Failed to create target/ directory: {}", e))?;
                std::path::PathBuf::from(format!("target/generated_{}_{}.{}", epoch, idx, ext))
            }
        };

        std::fs::write(&file_path, decoded)
            .map_err(|e| format!("Failed to write image file {:?}: {}", file_path, e))?;

        println!(
            "Downloaded and saved generated image to: {}",
            file_path.to_string_lossy()
        );
    }

    Ok(())
}

/// Display an image in the terminal using kitty's icat protocol.
/// Silently skips if kitty icat is not available.
fn display_image_in_terminal(image_path: &str) {
    let _ = Command::new("kitty").args(["icat", image_path]).status();
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct AttachmentUiState {
    indicator_count: usize,
    file_visible: bool,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct AttachmentUiOverview {
    indicator_count: usize,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct AttachmentReceipt {
    paths: Vec<String>,
    filename_confirmed_paths: Vec<String>,
    expected_indicator_count: usize,
}

impl AttachmentReceipt {
    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

fn copilot_attachment_upload_ready(before: &AttachmentUiState, after: &AttachmentUiState) -> bool {
    after.error.is_none()
        && !after.busy
        && before
            .indicator_count
            .checked_add(1)
            .is_some_and(|expected| after.indicator_count == expected)
}

fn validate_copilot_attachment_baseline(state: &AttachmentUiOverview) -> Result<(), String> {
    if let Some(error) = &state.error {
        return Err(format!(
            "Microsoft 365 Copilot reported an attachment UI error: {}",
            error
        ));
    }
    if state.busy {
        return Err(
            "Microsoft 365 Copilot has an attachment upload in progress. Wait for it to finish, remove any attachment manually, and retry."
                .to_string(),
        );
    }
    if state.indicator_count != 0 {
        return Err(format!(
            "Microsoft 365 Copilot already has {} attachment indicator(s). Remove every existing attachment manually before retrying; ask-bridge will not delete attachment chips automatically.",
            state.indicator_count
        ));
    }

    Ok(())
}

fn validate_copilot_attachment_receipt_overview(
    state: &AttachmentUiOverview,
    expected_indicator_count: usize,
) -> Result<(), String> {
    if let Some(error) = &state.error {
        return Err(error.clone());
    }
    if state.busy {
        return Err("Microsoft 365 Copilot attachments are still uploading".to_string());
    }
    if state.indicator_count != expected_indicator_count {
        return Err(format!(
            "Microsoft 365 Copilot attachment indicators changed before submission (expected exactly {}, found {})",
            expected_indicator_count, state.indicator_count
        ));
    }

    Ok(())
}

fn read_copilot_attachment_ui_value(
    config_path: &str,
    file_name: Option<&str>,
    file_stem: Option<&str>,
) -> Result<Value, String> {
    let file_name_json = serde_json::to_string(&file_name)
        .map_err(|e| format!("Failed to serialize attachment file name: {}", e))?;
    let file_stem_json = serde_json::to_string(&file_stem)
        .map_err(|e| format!("Failed to serialize attachment file stem: {}", e))?;
    let indicator_selector_json = serde_json::to_string(COPILOT_ATTACHMENT_INDICATOR_SELECTOR)
        .map_err(|e| format!("Failed to serialize attachment indicator selector: {}", e))?;

    let js = r#"() => {
        const composerSelectors = __COMPOSER_SELECTORS__;
        const requestedFileName = __FILE_NAME__;
        const requestedFileStem = __FILE_STEM__;
        const fileName = typeof requestedFileName === 'string'
            ? requestedFileName.toLocaleLowerCase()
            : '';
        const fileStem = typeof requestedFileStem === 'string'
            ? requestedFileStem.toLocaleLowerCase()
            : '';
        const candidateSelector = __ATTACHMENT_SELECTOR__;
        const isVisible = (el) => {
            if (!el) return false;
            const style = window.getComputedStyle(el);
            if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
            const rect = el.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0;
        };
        const composer = composerSelectors
            .flatMap((selector) => Array.from(document.querySelectorAll(selector)))
            .find(isVisible);
        if (!composer) {
            return {
                indicator_count: 0,
                file_visible: false,
                busy: false,
                error: 'Microsoft 365 Copilot composer not found while checking attachments'
            };
        }

        const root = composer.closest(
            'form, [data-testid*="composer-container" i], [data-testid*="composer-wrapper" i], [class*="ComposerWrapper"], [class*="ChatInputContainer"]'
        ) || composer.parentElement?.parentElement?.parentElement?.parentElement || composer.parentElement;
        if (!root) {
            return {
                indicator_count: 0,
                file_visible: false,
                busy: false,
                error: 'Microsoft 365 Copilot composer container not found while checking attachments'
            };
        }

        const rawCandidates = Array.from(root.querySelectorAll(candidateSelector)).filter((el) => {
            if (!isVisible(el) || el === composer || composer.contains(el)) return false;
            const label = [
                el.getAttribute('aria-label'),
                el.getAttribute('title'),
                el.getAttribute('data-testid'),
                el.textContent
            ].filter(Boolean).join(' ').toLocaleLowerCase();
            if (/add content|upload images|upload files|新增內容|上傳圖片|上傳檔案|添加内容|上传图像|上传文件/i.test(label)) {
                return false;
            }
            return true;
        });
        const seen = new Set();
        const candidates = [];
        for (const candidate of rawCandidates) {
            if (seen.has(candidate)) continue;
            seen.add(candidate);

            // A chip/card and its nested preview/remove button often match more than
            // one selector. Count the outermost matching attachment element once.
            if (candidates.some((existing) => existing.contains(candidate))) continue;
            for (let index = candidates.length - 1; index >= 0; index--) {
                if (candidate.contains(candidates[index])) candidates.splice(index, 1);
            }
            candidates.push(candidate);
        }
        const textFor = (el) => [
            el.getAttribute('aria-label'),
            el.getAttribute('title'),
            el.getAttribute('alt'),
            el.getAttribute('data-testid'),
            el.textContent
        ].filter(Boolean).join(' ').toLocaleLowerCase();
        const fileVisible = Boolean(fileName) && candidates.some((el) => {
            const text = textFor(el);
            return text.includes(fileName) || (fileStem && text.includes(fileStem));
        });
        const busy = Array.from(root.querySelectorAll(
            '[aria-busy="true"], [role="progressbar"], [data-testid*="upload-progress" i], [class*="UploadProgress"], [class*="upload-progress"]'
        )).some(isVisible);

        const alertText = Array.from(document.querySelectorAll(
            '[role="alert"], [role="status"], [data-testid*="toast" i], [class*="Toast"]'
        )).filter(isVisible).map((el) => (el.innerText || el.textContent || '').trim()).filter(Boolean).join('\n');
        const errorPatterns = [
            /(?:upload|attach|file|image)[^\n]{0,100}(?:failed|error|unsupported|not supported|unable|couldn['’]?t|too large|too many|blocked)/i,
            /(?:failed|error|unsupported|not supported|unable|couldn['’]?t|too large|too many|blocked)[^\n]{0,100}(?:upload|attach|file|image)/i,
            /(?:上傳|上传|附件|檔案|文件|圖片|图像)[^\n]{0,60}(?:失敗|失败|錯誤|错误|不支援|不支持|無法|无法|過大|过大|封鎖|阻止)/i
        ];
        const error = errorPatterns.some((pattern) => pattern.test(alertText)) ? alertText : null;

        return {
            indicator_count: candidates.length,
            file_visible: Boolean(fileVisible),
            busy: Boolean(busy),
            error
        };
    }"#
    .replace(
        "__COMPOSER_SELECTORS__",
        Provider::Copilot.composer_selectors_json(),
    )
    .replace("__ATTACHMENT_SELECTOR__", &indicator_selector_json)
    .replace("__FILE_NAME__", &file_name_json)
    .replace("__FILE_STEM__", &file_stem_json);

    let result = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    parse_script_result(&result)
}

fn read_copilot_attachment_ui_overview(config_path: &str) -> Result<AttachmentUiOverview, String> {
    let parsed = read_copilot_attachment_ui_value(config_path, None, None)?;
    serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse Copilot attachment UI overview: {}", e))
}

fn read_copilot_attachment_ui_state(
    config_path: &str,
    path: &str,
) -> Result<AttachmentUiState, String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    let file_stem = Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name);
    let parsed = read_copilot_attachment_ui_value(config_path, Some(file_name), Some(file_stem))?;
    serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse Copilot attachment UI state: {}", e))
}

fn ensure_copilot_attachment_baseline(config_path: &str) -> Result<(), String> {
    let overview = read_copilot_attachment_ui_overview(config_path)?;
    validate_copilot_attachment_baseline(&overview)
}

fn click_copilot_add_content_via_dom(config_path: &str) -> Result<(), String> {
    let js = r#"() => {
        const composerSelectors = __COMPOSER_SELECTORS__;
        const isVisible = (el) => {
            if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
            const style = window.getComputedStyle(el);
            if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
            const rect = el.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0;
        };
        const composer = composerSelectors
            .flatMap((selector) => Array.from(document.querySelectorAll(selector)))
            .find(isVisible);
        if (!composer) return { ok: false, error: 'composer not found' };
        const composerRect = composer.getBoundingClientRect();
        const nearComposer = (el) => {
            const rect = el.getBoundingClientRect();
            const horizontal = Math.abs((rect.left + rect.right) / 2 - (composerRect.left + composerRect.right) / 2);
            return horizontal <= Math.max(600, composerRect.width) &&
                rect.bottom >= composerRect.top - 250 && rect.top <= composerRect.bottom + 250;
        };
        const labelFor = (el) => [
            el.getAttribute('aria-label'),
            el.getAttribute('title'),
            el.getAttribute('data-testid'),
            el.textContent
        ].filter(Boolean).join(' ').replace(/\s+/g, ' ').trim();
        const candidates = Array.from(document.querySelectorAll('button, [role="button"]'))
            .filter((el) => isVisible(el) && nearComposer(el));
        const button = candidates.find((el) => {
            const label = labelFor(el);
            return /add content/i.test(label) || label.includes('新增內容') || label.includes('添加内容');
        }) || candidates.find((el) => /attach|add.?content/i.test(el.getAttribute('data-testid') || ''));
        if (!button) return { ok: false, error: 'Add content button not found' };
        const label = labelFor(button);
        button.click();
        return { ok: true, label };
    }"#
    .replace(
        "__COMPOSER_SELECTORS__",
        Provider::Copilot.composer_selectors_json(),
    );

    let result = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let parsed = parse_script_result(&result)?;
    if parsed
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(parsed
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("Add content button not found")
            .to_string())
    }
}

fn open_copilot_local_upload_menu(config_path: &str) -> Result<String, String> {
    let snapshot = take_snapshot_text(config_path)?;
    if let Some(upload_uid) = find_copilot_upload_images_and_files_uid(&snapshot) {
        return Ok(upload_uid);
    }

    let clicked = if let Some(add_content_uid) = find_copilot_add_content_uid(&snapshot) {
        call_mcp_tool(
            config_path,
            "click",
            serde_json::json!({
                "uid": add_content_uid,
                "includeSnapshot": false
            }),
        )
        .map(|_| ())
    } else {
        click_copilot_add_content_via_dom(config_path)
    };
    clicked.map_err(|e| {
        format!(
            "Could not open Microsoft 365 Copilot Add content menu: {}",
            e
        )
    })?;

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        thread::sleep(Duration::from_millis(200));
        let snapshot = take_snapshot_text(config_path)?;
        if let Some(upload_uid) = find_copilot_upload_images_and_files_uid(&snapshot) {
            return Ok(upload_uid);
        }
    }

    Err(
        "Microsoft 365 Copilot did not show 'Upload images and files' after opening Add content. The tenant license or company policy may disable local uploads."
            .to_string(),
    )
}

fn wait_for_copilot_attachment_upload(
    config_path: &str,
    path: &str,
    before: &AttachmentUiState,
    verbose: bool,
) -> Result<AttachmentUiState, String> {
    let started = Instant::now();
    let mut stable_successes = 0usize;
    let mut last_state = before.clone();

    while started.elapsed() < COPILOT_ATTACHMENT_UPLOAD_TIMEOUT {
        thread::sleep(COPILOT_ATTACHMENT_POLL_INTERVAL);
        let state = read_copilot_attachment_ui_state(config_path, path)?;
        if let Some(error) = &state.error {
            return Err(format!(
                "Microsoft 365 Copilot rejected attachment '{}': {}",
                path, error
            ));
        }

        if copilot_attachment_upload_ready(before, &state) {
            stable_successes += 1;
            if stable_successes >= 2 {
                if verbose {
                    println!(
                        "{} accepted attachment '{}'",
                        Provider::Copilot.display_name(),
                        Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(path)
                    );
                }
                return Ok(state);
            }
        } else {
            stable_successes = 0;
        }
        last_state = state;
    }

    Err(format!(
        "Timed out waiting for Microsoft 365 Copilot to finish uploading '{}'. Attachment indicators: {} -> {}; busy: {}",
        path, before.indicator_count, last_state.indicator_count, last_state.busy
    ))
}

fn upload_copilot_attachments_via_file_chooser(
    config_path: &str,
    image_paths: &[String],
    file_paths: &[String],
    verbose: bool,
) -> Result<AttachmentReceipt, String> {
    let mut receipt = AttachmentReceipt::default();

    for path in image_paths.iter().chain(file_paths.iter()) {
        if receipt.is_empty() {
            ensure_copilot_attachment_baseline(config_path)?;
        } else {
            ensure_copilot_attachment_receipt(config_path, &receipt)?;
        }

        let canonical_path = std::fs::canonicalize(path)
            .map_err(|e| format!("Failed to resolve file '{}': {}", path, e))?;
        let file_path = canonical_path.to_string_lossy().to_string();
        let before = read_copilot_attachment_ui_state(config_path, path)?;
        if let Some(error) = &before.error {
            return Err(error.clone());
        }

        let upload_uid = open_copilot_local_upload_menu(config_path)?;
        if verbose {
            println!(
                "Uploading attachment '{}' to {}...",
                file_path,
                Provider::Copilot.display_name()
            );
        }
        call_mcp_tool(
            config_path,
            "upload_file",
            serde_json::json!({
                "uid": upload_uid,
                "filePath": file_path,
                "includeSnapshot": false
            }),
        )?;

        let after = wait_for_copilot_attachment_upload(config_path, path, &before, verbose)?;
        receipt.paths.push(path.clone());
        receipt.expected_indicator_count = after.indicator_count;
        if after.file_visible {
            receipt.filename_confirmed_paths.push(path.clone());
        }
    }

    Ok(receipt)
}

fn ensure_copilot_attachment_receipt(
    config_path: &str,
    receipt: &AttachmentReceipt,
) -> Result<(), String> {
    if receipt.is_empty() {
        return Ok(());
    }

    let overview = read_copilot_attachment_ui_overview(config_path)?;
    validate_copilot_attachment_receipt_overview(&overview, receipt.expected_indicator_count)?;

    for path in &receipt.filename_confirmed_paths {
        let state = read_copilot_attachment_ui_state(config_path, path)?;
        if !state.file_visible {
            return Err(format!(
                "Microsoft 365 Copilot attachment '{}' disappeared before submission",
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
            ));
        }
    }

    Ok(())
}

fn wait_for_attachment_indicator(
    config_path: &str,
    provider: Provider,
    path: &str,
    verbose: bool,
) -> Result<(), String> {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    let file_stem = Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or(file_name);
    let file_name_json = serde_json::to_string(file_name)
        .map_err(|e| format!("Failed to serialize file name: {}", e))?;
    let file_stem_json = serde_json::to_string(file_stem)
        .map_err(|e| format!("Failed to serialize file stem: {}", e))?;
    let js = r#"() => {
        const fileName = __FILE_NAME__;
        const fileStem = __FILE_STEM__;
        const text = document.body.innerText || '';
        return text.includes(fileName) || text.includes(fileStem);
    }"#
    .replace("__FILE_NAME__", &file_name_json)
    .replace("__FILE_STEM__", &file_stem_json);

    for _ in 0..30 {
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        if parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_bool())
            .unwrap_or(false)
        {
            if verbose {
                println!(
                    "{} accepted attachment '{}'",
                    provider.display_name(),
                    file_name
                );
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Timed out waiting for {} to show attachment '{}'",
        provider.display_name(),
        file_name
    ))
}

fn upload_attachments_via_file_chooser(
    config_path: &str,
    provider: Provider,
    image_paths: &[String],
    file_paths: &[String],
    verbose: bool,
) -> Result<(), String> {
    for (path, verify_filename) in image_paths
        .iter()
        .map(|path| (path, false))
        .chain(file_paths.iter().map(|path| (path, true)))
    {
        let canonical_path = std::fs::canonicalize(path)
            .map_err(|e| format!("Failed to resolve file '{}': {}", path, e))?;
        let file_path = canonical_path.to_string_lossy().to_string();

        let snapshot = take_snapshot_text(config_path)?;
        let menu_uid = match provider {
            Provider::Copilot => find_copilot_add_content_uid(&snapshot),
            Provider::Gemini => {
                find_snapshot_uid(&snapshot, &["上傳與工具"], &["更多", "雲端", "drive"])
                    .or_else(|| find_snapshot_uid(&snapshot, &["upload"], &["drive"]))
            }
            Provider::ChatGpt => find_snapshot_uid(&snapshot, &["attach"], &["settings", "menu"]),
            Provider::Claude => find_snapshot_uid(&snapshot, &["attach"], &["settings", "menu"])
                .or_else(|| find_snapshot_uid(&snapshot, &["upload"], &["drive"])),
        }
        .ok_or_else(|| {
            format!(
                "Could not find {} upload menu in page snapshot",
                provider.display_name()
            )
        })?;

        call_mcp_tool(
            config_path,
            "click",
            serde_json::json!({
                "uid": menu_uid,
                "includeSnapshot": false
            }),
        )?;
        thread::sleep(Duration::from_millis(500));

        let snapshot = take_snapshot_text(config_path)?;
        let upload_uid = match provider {
            Provider::Copilot => find_copilot_upload_images_and_files_uid(&snapshot),
            Provider::Gemini => find_snapshot_uid(&snapshot, &["上傳檔案"], &["雲端", "drive"])
                .or_else(|| find_snapshot_uid(&snapshot, &["upload", "file"], &["drive"])),
            Provider::ChatGpt => find_snapshot_uid(&snapshot, &["file"], &["drive", "connect"]),
            Provider::Claude => {
                find_snapshot_uid(&snapshot, &["upload", "file"], &["drive", "connect"])
                    .or_else(|| find_snapshot_uid(&snapshot, &["file"], &["drive", "connect"]))
            }
        }
        .unwrap_or_else(|| menu_uid.clone());

        if verbose {
            println!(
                "Uploading attachment '{}' to {}...",
                file_path,
                provider.display_name()
            );
        }
        call_mcp_tool(
            config_path,
            "upload_file",
            serde_json::json!({
                "uid": upload_uid,
                "filePath": file_path,
                "includeSnapshot": false
            }),
        )?;
        if verify_filename {
            wait_for_attachment_indicator(config_path, provider, path, verbose)?;
        } else {
            thread::sleep(Duration::from_millis(800));
        }
    }

    Ok(())
}

/// Map a file extension to a MIME type. Covers common image and document formats.
/// `ext` is expected to already be lowercased by the caller.
fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docm" => "application/vnd.ms-word.document.macroEnabled.12",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppsm" => "application/vnd.ms-powerpoint.slideshow.macroEnabled.12",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "json" => "application/json",
        "yaml" | "yml" => "text/yaml",
        "ts" => "text/typescript",
        "tsx" => "text/typescript",
        "js" | "mjs" | "cjs" => "text/javascript",
        "jsx" => "text/javascript",
        "css" => "text/css",
        "dart" => "text/x-dart",
        "lua" => "text/x-lua",
        "pl" => "text/x-perl",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "rs" => "text/x-rust",
        "java" => "text/x-java",
        "kt" => "text/x-kotlin",
        "c" => "text/x-c",
        "h" => "text/x-c",
        "cpp" | "cc" | "cxx" => "text/x-c++",
        "hpp" => "text/x-c++",
        "cs" => "text/x-csharp",
        "swift" => "text/x-swift",
        "php" => "text/x-php",
        "sh" => "application/x-sh",
        "bash" => "application/x-sh",
        "zsh" => "application/x-sh",
        "sql" => "application/sql",
        "toml" => "application/toml",
        "ini" | "config" | "utf8" => "text/plain",
        "log" => "text/plain",
        "loop" => "application/vnd.microsoft.loop",
        "fluid" => "application/vnd.microsoft.fluid",
        // Archives
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        // Video
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Upload local image and/or document files to the provider prompt composer using the
/// best available provider-specific upload mechanism.
/// Returns a receipt that Copilot can re-check immediately before submission.
fn upload_attachments_to_provider(
    config_path: &str,
    provider: Provider,
    image_paths: &[String],
    file_paths: &[String],
    verbose: bool,
) -> Result<AttachmentReceipt, String> {
    let total = image_paths.len() + file_paths.len();
    if total == 0 {
        return Ok(AttachmentReceipt::default());
    }

    if provider == Provider::Copilot {
        return upload_copilot_attachments_via_file_chooser(
            config_path,
            image_paths,
            file_paths,
            verbose,
        );
    }

    let data_transfer_image_paths: &[String] = if provider == Provider::Gemini
        && !image_paths.is_empty()
    {
        match upload_attachments_via_file_chooser(config_path, provider, image_paths, &[], verbose)
        {
            Ok(()) => &[],
            Err(e) => {
                if verbose {
                    eprintln!(
                        "Warning: {} image file chooser upload failed, trying DataTransfer fallback: {}",
                        provider.display_name(),
                        e
                    );
                }
                image_paths
            }
        }
    } else {
        image_paths
    };

    let data_transfer_total = data_transfer_image_paths.len() + file_paths.len();
    if data_transfer_total == 0 {
        return Ok(AttachmentReceipt::default());
    }

    if verbose {
        println!(
            "Attaching {} attachment(s) ({} image(s), {} file(s)) to the prompt...",
            data_transfer_total,
            data_transfer_image_paths.len(),
            file_paths.len()
        );
    }

    // Build a JSON array of { name, mime, base64 } objects. Images first, then other files.
    // We pass raw base64 + mime and decode in JS to avoid `fetch(data:...)` which ChatGPT's
    // Content-Security-Policy blocks (results in "Failed to fetch").
    let mut files_json = Vec::new();
    for path in data_transfer_image_paths.iter().chain(file_paths.iter()) {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let mime = mime_type_for_extension(&ext);
        let b64 = general_purpose::STANDARD.encode(&bytes);
        let file_name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        files_json.push(serde_json::json!({
            "name": file_name,
            "mime": mime,
            "base64": b64
        }));
    }

    let files_json_str = serde_json::to_string(&files_json)
        .map_err(|e| format!("Failed to serialize attachment data: {}", e))?;
    let composer_selectors = provider.composer_selectors_json();
    // Build JS without raw strings to avoid r#"..."# termination conflicts
    let js = "() => {\n".to_string()
        + "    window.__upload_images_status = 'pending';\n"
        + "    (async () => {\n"
        + "        try {\n"
        + &format!("            const filesData = {};\n", files_json_str)
        + "            const decodeB64 = (b64) => {\n"
        + "                const bin = atob(b64);\n"
        + "                const len = bin.length;\n"
        + "                const bytes = new Uint8Array(len);\n"
        + "                for (let i = 0; i < len; i++) bytes[i] = bin.charCodeAt(i);\n"
        + "                return bytes;\n"
        + "            };\n"
        + "            const fileObjects = filesData.map((f) => {\n"
        + "                const bytes = decodeB64(f.base64);\n"
        + "                const blob = new Blob([bytes], { type: f.mime || 'application/octet-stream' });\n"
        + "                return new File([blob], f.name, { type: blob.type });\n"
        + "            });\n"
        + &format!(
            "            const composerSelectors = {};\n",
            composer_selectors
        )
        + "            const el = composerSelectors.map((s) => document.querySelector(s)).find(Boolean);\n"
        + "            if (!el) {\n"
        + "                window.__upload_images_status = 'error: composer not found';\n"
        + "                return;\n"
        + "            }\n"
        + "            el.focus();\n"
        + "            const fileInputs = Array.from(document.querySelectorAll('input[type=\"file\"]'));\n"
        + "            // Pick the file input whose `accept` attribute covers every attached file.\n"
        + "            // An input accepts a file when accept is empty, contains `*/*` or a matching\n"
        + "            // wildcard (e.g. `image/*`), or lists the file's exact MIME type.\n"
        + "            const accepts = (input, file) => {\n"
        + "                const acc = (input.getAttribute('accept') || '').trim();\n"
        + "                if (!acc) return true;\n"
        + "                const parts = acc.split(',').map(s => s.trim().toLowerCase()).filter(Boolean);\n"
        + "                const mime = (file.type || '').toLowerCase();\n"
        + "                const top = mime.split('/')[0];\n"
        + "                return parts.some(p => p === '*/*' || p === mime || (p.endsWith('/*') && top && p === top + '/*'));\n"
        + "            };\n"
        + "            const fileInput = fileInputs.find(i => fileObjects.every(f => accepts(i, f)))\n"
        + "                || fileInputs.find(i => !i.getAttribute('accept'))\n"
        + "                || fileInputs[0];\n"
        + "            if (fileInput) {\n"
        + "                const dt = new DataTransfer();\n"
        + "                for (const f of fileObjects) dt.items.add(f);\n"
        + "                fileInput.files = dt.files;\n"
        + "                fileInput.dispatchEvent(new Event('change', { bubbles: true }));\n"
        + "                window.__upload_images_status = 'success:file-input';\n"
        + "                return;\n"
        + "            }\n"
        + "            const dt = new DataTransfer();\n"
        + "            for (const f of fileObjects) dt.items.add(f);\n"
        + "            const targets = [el, el.closest('form'), document.querySelector('main'), document.body].filter(Boolean);\n"
        + "            for (const target of targets) {\n"
        + "                for (const type of ['dragenter', 'dragover', 'drop']) {\n"
        + "                    target.dispatchEvent(new DragEvent(type, {\n"
        + "                        bubbles: true, cancelable: true, dataTransfer: dt\n"
        + "                    }));\n"
        + "                }\n"
        + "            }\n"
        + "            const pasteEvent = new ClipboardEvent('paste', {\n"
        + "                bubbles: true, cancelable: true, clipboardData: dt\n"
        + "            });\n"
        + "            el.dispatchEvent(pasteEvent);\n"
        + "            window.__upload_images_status = 'success:drop';\n"
        + "        } catch (e) {\n"
        + "            window.__upload_images_status = 'error: ' + e.message;\n"
        + "        }\n"
        + "    })();\n"
        + "    return true;\n"
        + "}";

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate attachment upload script".to_string());
    }

    // Poll for completion. Allow up to ~60s for large document uploads.
    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 300 {
        thread::sleep(Duration::from_millis(200));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": "() => window.__upload_images_status || 'pending'" }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|r| r.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Attachment upload failed: {}", status));
    }
    if status == "pending" {
        return Err("Timed out waiting for attachments to upload".to_string());
    }

    if verbose {
        println!("Attachments attached successfully ({})", status);
    }

    // Give the UI a moment to render the attachments before typing the prompt
    thread::sleep(Duration::from_millis(800));

    if provider == Provider::Gemini {
        // Gemini renders image attachments as thumbnails without a stable filename in
        // the accessible text. Text/document chips do expose their filename, so keep
        // the stricter post-upload check for `--file` attachments only.
        for path in file_paths {
            if let Err(e) = wait_for_attachment_indicator(config_path, provider, path, verbose) {
                if verbose {
                    eprintln!(
                        "Warning: {} DataTransfer upload was not detected, trying file chooser fallback: {}",
                        provider.display_name(),
                        e
                    );
                }
                upload_attachments_via_file_chooser(
                    config_path,
                    provider,
                    image_paths,
                    file_paths,
                    verbose,
                )?;
                return Ok(AttachmentReceipt::default());
            }
        }
    }

    Ok(AttachmentReceipt::default())
}

/// Switch the selected provider to the specified model. The page must already be
/// loaded and logged in. `model` is matched case- and punctuation-insensitively.
fn switch_model(
    config_path: &str,
    provider: Provider,
    model: &str,
    verbose: bool,
) -> Result<(), String> {
    if provider == Provider::Copilot {
        return Err("Microsoft 365 Copilot model switching is not supported yet.".to_string());
    }

    if model.trim().is_empty() {
        return Err("Empty model name".to_string());
    }
    let target_json = serde_json::to_string(model.trim())
        .map_err(|e| format!("Failed to serialize model name: {}", e))?;

    if verbose {
        println!(
            "Switching {} model to '{}'...",
            provider.display_name(),
            model.trim()
        );
    }

    let js = match provider {
        Provider::ChatGpt => {
            // The script opens the composer pill menu, walks visible leaves and submenu
            // triggers, and clicks the first leaf whose normalized label matches.
            "() => {\n".to_string()
                + "    window.__switch_model_status = 'pending';\n"
                + "    (async () => {\n"
                + "    try {\n"
                + "        const sleep = (ms) => new Promise((r) => setTimeout(r, ms));\n"
                + "        const norm = (s) => (s || '').toLowerCase().replace(/[\\s.\\-_]/g, '');\n"
                + &format!("        const target = norm({});\n", target_json)
                + "        if (!target) { window.__switch_model_status = 'error: empty target'; return; }\n"
                + "        const visited = new Set();\n"
                + "        const closeMenus = async () => {\n"
                + "            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));\n"
                + "            await sleep(400);\n"
                + "        };\n"
                + "        await closeMenus();\n"
                + "        let pill = null;\n"
                + "        for (let i = 0; i < 20; i++) {\n"
                + "            pill = document.querySelector('button.__composer-pill');\n"
                + "            if (pill) break;\n"
                + "            await sleep(250);\n"
                + "        }\n"
                + "        if (!pill) { window.__switch_model_status = 'error: composer pill not found'; return; }\n"
                + "        pill.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));\n"
                + "        pill.dispatchEvent(new MouseEvent('pointerup', { bubbles: true }));\n"
                + "        pill.click();\n"
                + "        await sleep(800);\n"
                + "        let clicked = false;\n"
                + "        let chosen = '';\n"
                + "        for (let depth = 0; depth < 6 && !clicked; depth++) {\n"
                + "            const all = Array.from(document.querySelectorAll('[role=\"menuitem\"], [role=\"menuitemradio\"]'));\n"
                + "            const leaves = all.filter((it) => it.getAttribute('aria-haspopup') !== 'menu');\n"
                + "            for (const it of leaves) {\n"
                + "                const t = norm(it.innerText);\n"
                + "                if (t && t === target) {\n"
                + "                    it.click();\n"
                + "                    clicked = true;\n"
                + "                    chosen = it.innerText;\n"
                + "                    break;\n"
                + "                }\n"
                + "            }\n"
                + "            if (clicked) break;\n"
                + "            const trigs = all.filter((it) => it.getAttribute('aria-haspopup') === 'menu');\n"
                + "            const trig = trigs.find((it) => {\n"
                + "                const k = norm(it.innerText) + '|' + (it.getAttribute('aria-label') || '');\n"
                + "                return !visited.has(k);\n"
                + "            });\n"
                + "            if (!trig) break;\n"
                + "            visited.add(norm(trig.innerText) + '|' + (trig.getAttribute('aria-label') || ''));\n"
                + "            trig.dispatchEvent(new MouseEvent('pointerenter', { bubbles: true }));\n"
                + "            trig.dispatchEvent(new MouseEvent('pointermove', { bubbles: true }));\n"
                + "            trig.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));\n"
                + "            trig.click();\n"
                + "            await sleep(750);\n"
                + "        }\n"
                + "        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));\n"
                + "        if (!clicked) {\n"
                + "            window.__switch_model_status = 'error: model not found in menu';\n"
                + "            return;\n"
                + "        }\n"
                + "        window.__switch_model_status = 'success:' + chosen;\n"
                + "    } catch (e) {\n"
                + "        window.__switch_model_status = 'error: ' + e.message;\n"
                + "    }\n"
                + "    })();\n"
                + "    return true;\n"
                + "}"
        }
        Provider::Gemini => {
            let template = r#"() => {
                window.__switch_model_status = 'pending';
                (async () => {
                    try {
                        const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
                        const norm = (s) => (s || '').toLowerCase().replace(/[^\p{Letter}\p{Number}]+/gu, '');
                        const canonical = (s) => {
                            const n = norm(s).replace(/^已選取/, '');
                            if (n.includes('flashlite') || n.includes('31flashlite')) return 'flashlite';
                            if (n.includes('35flash') || (n.endsWith('flash') && !n.includes('lite'))) return 'flash';
                            if (n.includes('31pro') || n === 'pro') return 'pro';
                            return n;
                        };
                        const target = canonical(__TARGET_MODEL__);
                        if (!target) { window.__switch_model_status = 'error: empty target'; return; }
                        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));
                        await sleep(250);
                        const buttons = Array.from(document.querySelectorAll('button'));
                        const modeButton = buttons.find((button) => /模式挑選器|model picker|mode picker/i.test([
                            button.getAttribute('aria-label'),
                            button.textContent
                        ].filter(Boolean).join(' ')));
                        if (!modeButton) { window.__switch_model_status = 'error: Gemini mode picker not found'; return; }
                        modeButton.click();
                        await sleep(800);
                        const items = Array.from(document.querySelectorAll('[role="menuitem"], [role="menuitemradio"]'));
                        let chosen = null;
                        for (const item of items) {
                            const label = item.innerText || item.textContent || item.getAttribute('aria-label') || '';
                            if (canonical(label) === target || norm(label) === norm(__TARGET_MODEL__)) {
                                chosen = item;
                                break;
                            }
                        }
                        if (!chosen) {
                            window.__switch_model_status = 'error: model not found in menu';
                            return;
                        }
                        chosen.click();
                        await sleep(500);
                        window.__switch_model_status = 'success:' + (chosen.innerText || chosen.textContent || '').trim();
                    } catch (e) {
                        window.__switch_model_status = 'error: ' + e.message;
                    }
                })();
                return true;
            }"#;
            template.replace("__TARGET_MODEL__", &target_json)
        }
        Provider::Claude => {
            let template = r#"() => {
                window.__switch_model_status = 'pending';
                (async () => {
                    try {
                        const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
                        const norm = (s) => (s || '').toLowerCase().replace(/[\s.\-_]/g, '');
                        const labelOf = (el) => ((el.innerText || el.textContent || '').split('\n')[0] || '').trim();
                        const target = norm(__TARGET_MODEL__);
                        if (!target) { window.__switch_model_status = 'error: empty target'; return; }
                        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));
                        await sleep(300);
                        let trigger = document.querySelector('[data-testid="model-selector-dropdown"]');
                        if (!trigger) {
                            trigger = Array.from(document.querySelectorAll('button')).find((button) => {
                                const popup = button.getAttribute('aria-haspopup');
                                if (popup !== 'menu' && popup !== 'listbox') return false;
                                const label = [button.getAttribute('aria-label'), button.textContent].filter(Boolean).join(' ');
                                return /model|claude|opus|sonnet|haiku|fable/i.test(label);
                            });
                        }
                        if (!trigger) { window.__switch_model_status = 'error: Claude model selector not found'; return; }
                        trigger.click();
                        await sleep(800);
                        const visited = new Set();
                        let clicked = false;
                        let chosen = '';
                        for (let depth = 0; depth < 4 && !clicked; depth++) {
                            const items = Array.from(document.querySelectorAll('[role="menuitem"], [role="option"], [role="menuitemradio"]'));
                            const leaves = items.filter((it) => it.getAttribute('aria-haspopup') !== 'menu');
                            let match = leaves.find((it) => norm(labelOf(it)) === target);
                            if (!match) match = leaves.find((it) => norm(labelOf(it)).startsWith(target));
                            if (match) {
                                match.click();
                                clicked = true;
                                chosen = labelOf(match);
                                break;
                            }
                            const trigs = items.filter((it) => it.getAttribute('aria-haspopup') === 'menu');
                            const trig = trigs.find((it) => !visited.has(norm(it.innerText)));
                            if (!trig) break;
                            visited.add(norm(trig.innerText));
                            trig.dispatchEvent(new MouseEvent('pointerenter', { bubbles: true }));
                            trig.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
                            trig.click();
                            await sleep(700);
                        }
                        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', keyCode: 27, bubbles: true }));
                        if (!clicked) {
                            window.__switch_model_status = 'error: model not found in menu';
                            return;
                        }
                        await sleep(400);
                        window.__switch_model_status = 'success:' + chosen;
                    } catch (e) {
                        window.__switch_model_status = 'error: ' + e.message;
                    }
                })();
                return true;
            }"#;
            template.replace("__TARGET_MODEL__", &target_json)
        }
        Provider::Copilot => unreachable!("Copilot model switching is rejected above"),
    };

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate model switch script".to_string());
    }

    let mut wait_cycles = 0;
    let mut status = String::from("pending");
    while status == "pending" && wait_cycles < 60 {
        thread::sleep(Duration::from_millis(200));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": "() => window.__switch_model_status || 'pending'" }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|r| r.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(format!("Model switch failed: {}", status));
    }
    if status == "pending" {
        return Err("Timed out waiting for model switch".to_string());
    }

    if verbose {
        println!("Model switched successfully ({})", status);
    }

    // Give the UI a moment to settle after switching models
    thread::sleep(Duration::from_millis(500));

    Ok(())
}

fn wait_for_submit_status(config_path: &str) -> Result<String, String> {
    let mut wait_cycles = 0;
    let mut status = String::from("pending");

    // Page-side submission scripts may wait up to 15s for ChatGPT/Gemini to
    // enable the send button, so keep this host-side polling window longer.
    while status == "pending" && wait_cycles < 180 {
        thread::sleep(Duration::from_millis(100));
        let check_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": "() => window.__submit_status || 'pending'"
            }),
        )?;
        if let Some(s) = parse_script_result(&check_res)
            .ok()
            .and_then(|p| p.as_str().map(|str_ref| str_ref.to_string()))
        {
            status = s;
        }
        wait_cycles += 1;
    }

    if status.starts_with("error:") {
        return Err(status);
    }

    if status == "pending" {
        return Err("Timed out waiting for send button to activate and submit".to_string());
    }

    Ok(status)
}

fn focus_composer(config_path: &str, provider: Provider) -> Result<(), String> {
    let js = r#"() => {
            const composerSelectors = __COMPOSER_SELECTORS__;
            const el = composerSelectors
                .flatMap((selector) => Array.from(document.querySelectorAll(selector)))
                .find((candidate) => {
                    const style = window.getComputedStyle(candidate);
                    const rect = candidate.getBoundingClientRect();
                    return style.display !== 'none' &&
                        style.visibility !== 'hidden' &&
                        style.opacity !== '0' &&
                        rect.width > 0 && rect.height > 0;
                });
            if (!el) {
                return { ok: false, error: 'visible composer not found' };
            }

            el.focus({ preventScroll: true });
            return { ok: document.activeElement === el };
        }"#
    .replace("__COMPOSER_SELECTORS__", provider.composer_selectors_json());

    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let parsed = parse_script_result(&res)?;
    if parsed
        .get("ok")
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(parsed
            .get("error")
            .and_then(|err| err.as_str())
            .unwrap_or("failed to focus composer")
            .to_string())
    }
}

fn visible_composer_is_empty(config_path: &str, provider: Provider) -> Result<bool, String> {
    let js = r#"() => {
            const composerSelectors = __COMPOSER_SELECTORS__;
            const el = composerSelectors
                .flatMap((selector) => Array.from(document.querySelectorAll(selector)))
                .find((candidate) => {
                    const style = window.getComputedStyle(candidate);
                    const rect = candidate.getBoundingClientRect();
                    return style.display !== 'none' &&
                        style.visibility !== 'hidden' &&
                        style.opacity !== '0' &&
                        rect.width > 0 && rect.height > 0;
                });
            if (!el) {
                return { ok: false, error: 'visible composer not found' };
            }

            const rawText = typeof el.value !== 'undefined'
                ? el.value
                : (el.innerText || el.textContent || '');
            const normalizedText = String(rawText)
                .replace(/[\u200B-\u200D\uFEFF]/g, '')
                .replace(/\u00A0/g, ' ')
                .trim();
            return {
                ok: true,
                empty: normalizedText.length === 0,
                textLength: normalizedText.length
            };
        }"#
    .replace("__COMPOSER_SELECTORS__", provider.composer_selectors_json());

    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({ "function": js }),
    )?;
    let parsed = parse_script_result(&res)?;
    if !parsed
        .get("ok")
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false)
    {
        return Err(parsed
            .get("error")
            .and_then(|err| err.as_str())
            .unwrap_or("failed to read visible composer state")
            .to_string());
    }

    parsed
        .get("empty")
        .and_then(|empty| empty.as_bool())
        .ok_or_else(|| "visible composer state did not include an empty flag".to_string())
}

fn press_composer_key(config_path: &str, key: &str) -> Result<(), String> {
    call_mcp_tool(
        config_path,
        "press_key",
        serde_json::json!({
            "key": key,
            "includeSnapshot": false
        }),
    )?;
    Ok(())
}

fn wait_for_visible_composer_empty(config_path: &str, provider: Provider) -> Result<bool, String> {
    for _ in 0..20 {
        if visible_composer_is_empty(config_path, provider)? {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

fn select_all_shortcut(is_macos: bool) -> &'static str {
    if is_macos { "Meta+A" } else { "Control+A" }
}

fn focus_and_clear_composer(config_path: &str, provider: Provider) -> Result<(), String> {
    // React-controlled textareas can ignore synthetic value/input mutations when
    // their internal value tracker observes the same value. Drive the focused
    // composer through Chrome DevTools' trusted keyboard path instead.
    let select_all_key = select_all_shortcut(cfg!(target_os = "macos"));

    focus_composer(config_path, provider)?;
    press_composer_key(config_path, select_all_key)?;
    press_composer_key(config_path, "Backspace")?;
    if wait_for_visible_composer_empty(config_path, provider)? {
        return Ok(());
    }

    // Some rich-text editors only honor Delete for the active selection. Retry
    // with a fresh trusted selection, then verify the DOM before continuing.
    focus_composer(config_path, provider)?;
    press_composer_key(config_path, select_all_key)?;
    press_composer_key(config_path, "Delete")?;
    if wait_for_visible_composer_empty(config_path, provider)? {
        Ok(())
    } else {
        Err("visible composer still contained text after trusted clear keys".to_string())
    }
}

fn wait_for_chatgpt_agent_menu(config_path: &str) -> Result<(), String> {
    let js = r#"() => {
            const isVisible = (el) => {
                if (!el) return false;
                const style = window.getComputedStyle(el);
                if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0;
            };
            const composer = document.querySelector('#prompt-textarea');
            const composerRect = composer ? composer.getBoundingClientRect() : null;
            const isNearComposer = (el) => {
                if (!composerRect) return true;
                const rect = el.getBoundingClientRect();
                const itemCenterX = (rect.left + rect.right) / 2;
                const composerCenterX = (composerRect.left + composerRect.right) / 2;
                const maxHorizontalDistance = Math.max(500, composerRect.width);
                return Math.abs(itemCenterX - composerCenterX) <= maxHorizontalDistance &&
                    Math.abs(rect.top - composerRect.bottom) <= 500;
            };
            const items = Array.from(document.querySelectorAll(
                '.popover .__menu-item, [class*="popover"] .__menu-item, [role="menuitem"], [role="option"], [cmdk-item]'
            ))
                .filter((el) => isVisible(el) && isNearComposer(el))
                .map((el) => (el.innerText || el.textContent || '').trim())
                .filter(Boolean);

            return { ok: items.length > 0, items: items.slice(0, 5) };
        }"#;

    let mut last_state = String::new();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(125));
        let res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        let parsed = parse_script_result(&res)?;
        if parsed
            .get("ok")
            .and_then(|ok| ok.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }
        last_state = parsed.to_string();
    }

    Err(format!(
        "Timed out waiting for ChatGPT agent menu after typing mention ({})",
        last_state
    ))
}

fn wait_for_chatgpt_agent_selection(config_path: &str) -> Result<(), String> {
    let js = r#"() => {
            const composer = document.querySelector('#prompt-textarea');
            if (!composer) {
                return { ok: false, error: 'composer not found' };
            }
            const agentPill = composer.querySelector(
                '[data-id="agent"], [data-system-hint-type="agent"], [data-symbol="ecosystemMention"], [data-inline-selection-pill][contenteditable="false"]'
            );
            return {
                ok: Boolean(agentPill),
                text: (composer.innerText || composer.textContent || '').trim(),
                keyword: agentPill ? (agentPill.getAttribute('data-keyword') || agentPill.textContent || '').trim() : ''
            };
        }"#;

    let mut last_state = String::new();
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(125));
        let res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({ "function": js }),
        )?;
        let parsed = parse_script_result(&res)?;
        if parsed
            .get("ok")
            .and_then(|ok| ok.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }
        last_state = parsed.to_string();
    }

    Err(format!(
        "Timed out waiting for ChatGPT agent selection after Tab ({})",
        last_state
    ))
}

fn submit_regular_prompt(
    config_path: &str,
    provider: Provider,
    prompt: &str,
) -> Result<String, String> {
    let prompt_json = serde_json::to_string(prompt)
        .map_err(|e| format!("Failed to serialize prompt text: {}", e))?;
    let set_and_submit_js = r#"() => {
            window.__submit_status = 'pending';
            (async () => {
                try {
                    const composerSelectors = __COMPOSER_SELECTORS__;
                    const sendSelectors = __SEND_SELECTORS__;
                    const el = composerSelectors.map((s) => document.querySelector(s)).find(Boolean);
                    if (!el) {
                        window.__submit_status = 'error: composer not found';
                        return;
                    }
                    el.focus();
                    
                    const value = __PROMPT__;
                    el.focus();
                    
                    try {
                        const range = document.createRange();
                        range.selectNodeContents(el);
                        const sel = window.getSelection();
                        sel.removeAllRanges();
                        sel.addRange(range);
                    } catch (e) {}
                    
                    let pasted = false;
                    try {
                        const dataTransfer = new DataTransfer();
                        dataTransfer.setData('text/plain', value);
                        const event = new ClipboardEvent('paste', {
                            bubbles: true,
                            cancelable: true
                        });
                        Object.defineProperty(event, 'clipboardData', {
                            value: dataTransfer,
                            writable: false,
                            configurable: true
                        });
                        el.dispatchEvent(event);
                        
                        const currentText = typeof el.value !== 'undefined' ? el.value : el.textContent;
                        if (currentText && currentText.trim().length > 0) {
                            pasted = true;
                        }
                    } catch (e) {}
                    
                    if (!pasted) {
                        const success = document.execCommand('insertText', false, value);
                        if (!success) {
                            if (typeof el.value !== 'undefined') {
                                el.value = value;
                                if (el._valueTracker) {
                                    el._valueTracker.setValue('');
                                }
                            } else {
                                el.innerText = value;
                            }
                            el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: value }));
                            el.dispatchEvent(new Event('change', { bubbles: true }));
                        }
                    }
                    
                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const findAndClickSendButton = () => {
                        let btn = null;
                        for (const s of sendSelectors) {
                            btn = document.querySelector(s);
                            if (isVisible(btn)) break;
                        }
                        
                        if (btn && !btn.disabled && btn.getAttribute('aria-disabled') !== 'true') {
                            btn.click();
                            return { ok: true, clicked: true, buttonLabel: btn.getAttribute('aria-label') };
                        }
                        return null;
                    };
                    
                    let result = findAndClickSendButton();
                    if (result) {
                        window.__submit_status = 'success:' + JSON.stringify(result);
                        return;
                    }

                    for (let i = 0; i < 150; i++) {
                        await new Promise(r => setTimeout(r, 100));
                        result = findAndClickSendButton();
                        if (result) {
                            window.__submit_status = 'success:' + JSON.stringify(result);
                            return;
                        }
                    }
                    
                    window.__submit_status = 'error: Send button did not become active/enabled';
                } catch (e) {
                    window.__submit_status = 'error: ' + e.message;
                }
            })();
            return true;
        }"#
    .replace("__COMPOSER_SELECTORS__", provider.composer_selectors_json())
    .replace("__SEND_SELECTORS__", provider.send_button_selectors_json())
    .replace("__PROMPT__", &prompt_json);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": set_and_submit_js
        }),
    )?;

    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate text entry and submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn expected_copilot_attachment_count(receipt: &AttachmentReceipt) -> usize {
    if receipt.is_empty() {
        0
    } else {
        receipt.expected_indicator_count
    }
}

fn build_copilot_click_send_js(expected_indicator_count: usize) -> Result<String, String> {
    let attachment_selector_json = serde_json::to_string(COPILOT_ATTACHMENT_INDICATOR_SELECTOR)
        .map_err(|e| format!("Failed to serialize attachment indicator selector: {}", e))?;

    Ok(r#"() => {
            window.__submit_status = 'pending';
            window.__ask_bridge_generation_seen = false;
            (async () => {
                try {
                    const composerSelectors = __COMPOSER_SELECTORS__;
                    const sendSelectors = __SEND_SELECTORS__;
                    const attachmentSelector = __ATTACHMENT_SELECTOR__;
                    const expectedAttachmentCount = __EXPECTED_ATTACHMENT_COUNT__;
                    const isVisible = (el) => {
                        if (!el) return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const composer = composerSelectors
                        .flatMap((selector) => Array.from(document.querySelectorAll(selector)))
                        .find(isVisible);
                    if (!composer) {
                        window.__submit_status = 'error: composer not found after typing';
                        return;
                    }

                    const composerText = typeof composer.value !== 'undefined'
                        ? composer.value
                        : (composer.innerText || composer.textContent || '');
                    if (!composerText.trim()) {
                        window.__submit_status = 'error: Copilot composer remained empty after typing';
                        return;
                    }

                    const attachmentRoot = composer.closest(
                        'form, [data-testid*="composer-container" i], [data-testid*="composer-wrapper" i], [class*="ComposerWrapper"], [class*="ChatInputContainer"]'
                    ) || composer.parentElement?.parentElement?.parentElement?.parentElement || composer.parentElement;
                    const readAttachmentState = () => {
                        if (!attachmentRoot) {
                            return { ok: false, indicatorCount: 0, busy: false };
                        }
                        const rawCandidates = Array.from(
                            attachmentRoot.querySelectorAll(attachmentSelector)
                        ).filter((el) => {
                            if (!isVisible(el) || el === composer || composer.contains(el)) return false;
                            const label = [
                                el.getAttribute('aria-label'),
                                el.getAttribute('title'),
                                el.getAttribute('data-testid'),
                                el.textContent
                            ].filter(Boolean).join(' ').toLocaleLowerCase();
                            return !/add content|upload images|upload files|新增內容|上傳圖片|上傳檔案|添加内容|上传图像|上传文件/i.test(label);
                        });
                        const seen = new Set();
                        const candidates = [];
                        for (const candidate of rawCandidates) {
                            if (seen.has(candidate)) continue;
                            seen.add(candidate);
                            if (candidates.some((existing) => existing.contains(candidate))) continue;
                            for (let index = candidates.length - 1; index >= 0; index--) {
                                if (candidate.contains(candidates[index])) candidates.splice(index, 1);
                            }
                            candidates.push(candidate);
                        }
                        const busy = Array.from(attachmentRoot.querySelectorAll(
                            '[aria-busy="true"], [role="progressbar"], [data-testid*="upload-progress" i], [class*="UploadProgress"], [class*="upload-progress"]'
                        )).some(isVisible);
                        return {
                            ok: true,
                            indicatorCount: candidates.length,
                            busy: Boolean(busy)
                        };
                    };

                    const isClickable = (el) =>
                        Boolean(el) &&
                        !el.disabled &&
                        el.getAttribute('aria-disabled') !== 'true' &&
                        isVisible(el);
                    const composerForm = composer.closest('form');
                    const composerWrapper = composer.closest(
                        '[data-testid*="composer"], [data-testid*="chat-input"], [class*="Composer"], [class*="ChatInput"], [class*="PromptInput"]'
                    );
                    const composerRect = composer.getBoundingClientRect();
                    const isNearComposer = (button) => {
                        const rect = button.getBoundingClientRect();
                        const verticalDistance = Math.max(
                            0,
                            composerRect.top - rect.bottom,
                            rect.top - composerRect.bottom
                        );
                        const horizontalOverlap = rect.right >= composerRect.left - 160 &&
                            rect.left <= composerRect.right + 160;
                        return horizontalOverlap && verticalDistance <= 160;
                    };
                    const belongsToComposer = (button) =>
                        Boolean(composerForm && composerForm.contains(button)) ||
                        Boolean(composerWrapper && composerWrapper.contains(button)) ||
                        isNearComposer(button);
                    const findSendButton = () => {
                        const seen = new Set();
                        for (const selector of sendSelectors) {
                            for (const button of document.querySelectorAll(selector)) {
                                if (seen.has(button)) continue;
                                seen.add(button);
                                if (isClickable(button) && belongsToComposer(button)) return button;
                            }
                        }
                        return null;
                    };

                    for (let i = 0; i < 150; i++) {
                        const button = findSendButton();
                        if (button) {
                            // This guard and click deliberately share one synchronous
                            // JavaScript turn so the DOM cannot update between them.
                            const attachmentState = readAttachmentState();
                            if (!attachmentState.ok) {
                                window.__submit_status = 'error: Copilot attachment container not found immediately before submission';
                                return;
                            }
                            if (attachmentState.busy) {
                                window.__submit_status = 'error: Copilot attachment upload became busy immediately before submission';
                                return;
                            }
                            if (attachmentState.indicatorCount !== expectedAttachmentCount) {
                                window.__submit_status = 'error: Copilot attachment indicators changed immediately before submission (expected ' +
                                    expectedAttachmentCount + ', found ' + attachmentState.indicatorCount + ')';
                                return;
                            }
                            button.click();
                            window.__submit_status = 'success:' + JSON.stringify({
                                clicked: true,
                                buttonLabel: button.getAttribute('aria-label'),
                                buttonType: button.getAttribute('type')
                            });
                            return;
                        }
                        await new Promise((resolve) => setTimeout(resolve, 100));
                    }

                    window.__submit_status = 'error: Copilot send button did not become active/enabled';
                } catch (e) {
                    window.__submit_status = 'error: ' + e.message;
                }
            })();
            return true;
        }"#
    .replace(
        "__COMPOSER_SELECTORS__",
        Provider::Copilot.composer_selectors_json(),
    )
    .replace(
        "__SEND_SELECTORS__",
        Provider::Copilot.send_button_selectors_json(),
    )
    .replace("__ATTACHMENT_SELECTOR__", &attachment_selector_json)
    .replace(
        "__EXPECTED_ATTACHMENT_COUNT__",
        &expected_indicator_count.to_string(),
    ))
}

fn submit_copilot_prompt(
    config_path: &str,
    prompt: &str,
    attachment_receipt: &AttachmentReceipt,
    verbose: bool,
) -> Result<String, String> {
    if verbose {
        println!("Typing prompt through the Microsoft 365 Copilot composer...");
    }

    // Copilot's React composer handles trusted keyboard input reliably. Synthetic
    // paste events can insert the prompt twice without updating the send-button
    // state, so use the same browser typing path as the ChatGPT agent workflow.
    if attachment_receipt.is_empty() {
        ensure_copilot_attachment_baseline(config_path)?;
        focus_and_clear_composer(config_path, Provider::Copilot)?;
        ensure_copilot_attachment_baseline(config_path)?;
    } else {
        ensure_copilot_attachment_receipt(config_path, attachment_receipt)?;
        // File upload leaves browser focus on the picker/menu. Restore the
        // composer without clearing it so the uploaded attachment chips remain.
        focus_composer(config_path, Provider::Copilot)?;
    }
    call_mcp_tool(
        config_path,
        "type_text",
        serde_json::json!({
            "text": prompt
        }),
    )?;
    if attachment_receipt.is_empty() {
        // Fail closed immediately before clicking Send. A stale chip appearing
        // while trusted text entry ran must never be mixed into this request.
        ensure_copilot_attachment_baseline(config_path)?;
    } else {
        ensure_copilot_attachment_receipt(config_path, attachment_receipt)?;
    }

    let click_send_js =
        build_copilot_click_send_js(expected_copilot_attachment_count(attachment_receipt))?;

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": click_send_js
        }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate Copilot prompt submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn submit_chatgpt_agent_prompt(
    config_path: &str,
    parts: &ChatGptAgentPrompt<'_>,
    verbose: bool,
) -> Result<String, String> {
    if verbose {
        println!(
            "Selecting ChatGPT agent '{}' before submitting prompt...",
            parts.agent_mention
        );
    }

    focus_and_clear_composer(config_path, Provider::ChatGpt)?;
    call_mcp_tool(
        config_path,
        "type_text",
        serde_json::json!({
            "text": parts.agent_mention
        }),
    )?;
    wait_for_chatgpt_agent_menu(config_path)?;
    call_mcp_tool(
        config_path,
        "press_key",
        serde_json::json!({
            "key": "Tab",
            "includeSnapshot": false
        }),
    )?;
    wait_for_chatgpt_agent_selection(config_path)?;

    let body_json = serde_json::to_string(parts.body)
        .map_err(|e| format!("Failed to serialize prompt body: {}", e))?;
    let paste_and_submit_js = r#"() => {
            window.__submit_status = 'pending';
            (async () => {
                try {
                    const sendSelectors = __SEND_SELECTORS__;
                    const el = document.querySelector('#prompt-textarea');
                    if (!el) {
                        window.__submit_status = 'error: composer not found';
                        return;
                    }
                    const agentPill = el.querySelector(
                        '[data-id="agent"], [data-system-hint-type="agent"], [data-symbol="ecosystemMention"], [data-inline-selection-pill][contenteditable="false"]'
                    );
                    if (!agentPill) {
                        window.__submit_status = 'error: ChatGPT agent was not selected into the composer';
                        return;
                    }

                    const body = __BODY__;
                    const currentText = el.textContent || '';
                    const value = currentText && !/\s$/.test(currentText) ? ' ' + body : body;
                    el.focus();

                    try {
                        const range = document.createRange();
                        range.selectNodeContents(el);
                        range.collapse(false);
                        const sel = window.getSelection();
                        sel.removeAllRanges();
                        sel.addRange(range);
                    } catch (e) {}

                    let pasted = false;
                    try {
                        const dataTransfer = new DataTransfer();
                        dataTransfer.setData('text/plain', value);
                        const event = new ClipboardEvent('paste', {
                            bubbles: true,
                            cancelable: true
                        });
                        Object.defineProperty(event, 'clipboardData', {
                            value: dataTransfer,
                            writable: false,
                            configurable: true
                        });
                        el.dispatchEvent(event);
                        const afterPasteText = el.innerText || el.textContent || '';
                        pasted = afterPasteText.includes(body);
                    } catch (e) {}

                    if (!pasted) {
                        const success = document.execCommand('insertText', false, value);
                        if (!success) {
                            el.appendChild(document.createTextNode(value));
                            el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: value }));
                            el.dispatchEvent(new Event('change', { bubbles: true }));
                        }
                    }

                    const afterText = el.innerText || el.textContent || '';
                    if (!afterText.includes(body)) {
                        window.__submit_status = 'error: prompt body was not pasted after ChatGPT agent selection';
                        return;
                    }

                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const findAndClickSendButton = () => {
                        let btn = null;
                        for (const s of sendSelectors) {
                            btn = document.querySelector(s);
                            if (isVisible(btn)) break;
                        }
                        if (btn && !btn.disabled && btn.getAttribute('aria-disabled') !== 'true') {
                            btn.click();
                            return { ok: true, clicked: true, buttonLabel: btn.getAttribute('aria-label') };
                        }
                        return null;
                    };

                    let result = findAndClickSendButton();
                    if (result) {
                        window.__submit_status = 'success:' + JSON.stringify(result);
                        return;
                    }

                    for (let i = 0; i < 150; i++) {
                        await new Promise(r => setTimeout(r, 100));
                        result = findAndClickSendButton();
                        if (result) {
                            window.__submit_status = 'success:' + JSON.stringify(result);
                            return;
                        }
                    }

                    window.__submit_status = 'error: Send button did not become active/enabled';
                } catch (e) {
                    window.__submit_status = 'error: ' + e.message;
                }
            })();
            return true;
        }"#
    .replace(
        "__SEND_SELECTORS__",
        Provider::ChatGpt.send_button_selectors_json(),
    )
    .replace("__BODY__", &body_json);

    let start_res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": paste_and_submit_js
        }),
    )?;
    let start_parsed = parse_script_result(&start_res)?;
    if !start_parsed.as_bool().unwrap_or(false) {
        return Err("Failed to initiate ChatGPT agent prompt submission script".to_string());
    }

    wait_for_submit_status(config_path)
}

fn submit_prompt_to_provider(
    config_path: &str,
    provider: Provider,
    prompt: &str,
    attachment_receipt: &AttachmentReceipt,
    verbose: bool,
) -> Result<String, String> {
    if provider == Provider::ChatGpt
        && let Some(parts) = parse_chatgpt_agent_prompt(prompt)
    {
        return submit_chatgpt_agent_prompt(config_path, &parts, verbose);
    }

    if provider == Provider::Copilot {
        return submit_copilot_prompt(config_path, prompt, attachment_receipt, verbose);
    }

    submit_regular_prompt(config_path, provider, prompt)
}

fn ensure_provider_tab(
    config_path: &str,
    provider: Provider,
    force_new: bool,
    headless: bool,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        println!("Checking open Chrome tabs...");
    }
    let list_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;

    let text = list_res
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|obj| obj.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("Invalid list_pages response structure: {:?}", list_res))?;

    let pages = parse_pages(text);

    if force_new {
        let old_provider_ids: Vec<usize> = pages
            .iter()
            .filter(|p| provider.owns_url(&p.url))
            .map(|p| p.id)
            .collect();

        if verbose {
            println!("Opening a brand new {} session...", provider.display_name());
        }
        call_mcp_tool(
            config_path,
            "new_page",
            serde_json::json!({
                "url": provider.home_url()
            }),
        )?;

        for id in old_provider_ids {
            if verbose {
                println!(
                    "Closing old {} tab (ID: {})...",
                    provider.display_name(),
                    id
                );
            }
            let _ = call_mcp_tool(
                config_path,
                "close_page",
                serde_json::json!({
                    "pageId": id
                }),
            );
        }

        let refreshed_pages_res = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))?;
        let refreshed_text = refreshed_pages_res
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                format!(
                    "Invalid refreshed list_pages response structure: {:?}",
                    refreshed_pages_res
                )
            })?;
        let refreshed_pages = parse_pages(refreshed_text);

        if let Some(page) = refreshed_pages.iter().find(|p| provider.owns_url(&p.url)) {
            if verbose {
                println!(
                    "Selecting new {} tab (ID: {})...",
                    provider.display_name(),
                    page.id
                );
            }
            call_mcp_tool(
                config_path,
                "select_page",
                serde_json::json!({
                    "pageId": page.id,
                    "bringToFront": !headless
                }),
            )?;

            for stale_page in refreshed_pages.iter().filter(|p| p.id != page.id) {
                if verbose {
                    println!("Closing non-selected tab (ID: {})...", stale_page.id);
                }
                let _ = call_mcp_tool(
                    config_path,
                    "close_page",
                    serde_json::json!({
                        "pageId": stale_page.id
                    }),
                );
            }
        }
    } else {
        let provider_pages: Vec<&Page> = pages
            .iter()
            .filter(|page| provider.owns_url(&page.url))
            .collect();

        let provider_page_id = if provider_pages.len() > 1 {
            let mut page_states = Vec::with_capacity(provider_pages.len());
            for page in &provider_pages {
                call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": false
                    }),
                )?;
                let login_state = check_login_status(config_path, provider, verbose)
                    .unwrap_or(LoginState::Unknown);
                page_states.push(PageLoginState {
                    id: page.id,
                    selected: page.selected,
                    login_state,
                });
            }
            preferred_provider_page_id(&page_states)
        } else {
            provider_pages.first().map(|page| page.id)
        };

        match provider_page_id {
            Some(page_id) => {
                let page = provider_pages
                    .iter()
                    .find(|page| page.id == page_id)
                    .ok_or_else(|| "Selected provider page disappeared".to_string())?;
                if verbose {
                    println!(
                        "Found {} tab (ID: {}, selected: {}). Selecting/focusing...",
                        provider.display_name(),
                        page.id,
                        page.selected
                    );
                }
                call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": !headless
                    }),
                )?;
            }
            None => {
                // No provider tab. If there is only one blank tab, navigate it. Otherwise open a new page.
                if pages.len() == 1
                    && (pages[0].url == "about:blank"
                        || pages[0].url.contains("new-tab-page")
                        || pages[0].url.contains("chrome://welcome"))
                {
                    if verbose {
                        println!(
                            "Navigating existing blank tab to {}...",
                            provider.display_name()
                        );
                    }
                    call_mcp_tool(
                        config_path,
                        "navigate_page",
                        serde_json::json!({
                            "url": provider.home_url()
                        }),
                    )?;
                } else {
                    if verbose {
                        println!("Opening a new tab for {}...", provider.display_name());
                    }
                    call_mcp_tool(
                        config_path,
                        "new_page",
                        serde_json::json!({
                            "url": provider.home_url()
                        }),
                    )?;
                }
            }
        }
    }

    // Wait for the provider composer to be present.
    if verbose {
        println!("Waiting for {} to load...", provider.display_name());
    }
    for attempt in 0..90 {
        if attempt > 0 && attempt % 10 == 0 {
            let page_opt = call_mcp_tool(config_path, "list_pages", serde_json::json!({}))
                .ok()
                .and_then(|res| {
                    res.get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|obj| obj.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string())
                })
                .and_then(|text| {
                    parse_pages(&text)
                        .into_iter()
                        .find(|p| provider.owns_url(&p.url))
                });
            if let Some(page) = page_opt {
                let _ = call_mcp_tool(
                    config_path,
                    "select_page",
                    serde_json::json!({
                        "pageId": page.id,
                        "bringToFront": !headless
                    }),
                );
            }
        }

        let ready_res = call_mcp_tool(
            config_path,
            "evaluate_script",
            serde_json::json!({
                "function": provider.ready_check_js()
            }),
        );
        let ready_res = match ready_res {
            Ok(res) => res,
            Err(e) => {
                if verbose {
                    eprintln!(
                        "Warning: Failed to check {} readiness: {}",
                        provider.display_name(),
                        e
                    );
                }
                thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        if let Ok(parsed) = parse_script_result(&ready_res) {
            let is_ready = parsed.as_bool().unwrap_or(false);
            if is_ready {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    Err(format!(
        "Timeout waiting for {} page to load",
        provider.display_name()
    ))
}

fn check_login_status(
    config_path: &str,
    provider: Provider,
    verbose: bool,
) -> Result<LoginState, String> {
    let res = call_mcp_tool(
        config_path,
        "evaluate_script",
        serde_json::json!({
            "function": provider.login_signals_js()
        }),
    )?;

    let parsed = parse_script_result(&res)?;
    let signals: LoginSignals = serde_json::from_value(parsed)
        .map_err(|e| format!("Failed to parse login signals: {}", e))?;
    if verbose {
        println!(
            "{} login signals: account={}, auth_control={}, auth_path={}, composer={}, stable={}",
            provider.display_name(),
            signals.account,
            signals.auth_control,
            signals.auth_path,
            signals.composer,
            signals.stable
        );
    }
    Ok(signals.state(provider))
}

fn wait_for_login_completion(
    config_path: &str,
    provider: Provider,
    timeout_seconds: u64,
    verbose: bool,
) -> (LoginState, bool) {
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let start = Instant::now();
    let display_name = provider.display_name();

    if verbose {
        println!(
            "Waiting for {} login status every second (timeout: {} seconds)...",
            display_name,
            timeout_seconds.max(1)
        );
    } else {
        println!("Waiting for login completion (checking every second)...");
    }

    loop {
        let state = match check_login_status(config_path, provider, verbose) {
            Ok(state) => state,
            Err(e) => {
                if verbose {
                    println!(
                        "Warning: Failed to check {} login status: {}",
                        display_name, e
                    );
                }
                LoginState::Unknown
            }
        };

        if state == LoginState::LoggedIn {
            return (LoginState::LoggedIn, false);
        }

        if start.elapsed() >= timeout {
            return (state, true);
        }

        thread::sleep(Duration::from_secs(1));
    }
}

fn complete_query_login_with<ShowLogin, WaitForLogin>(
    provider: Provider,
    timeout_seconds: u64,
    mut show_login: ShowLogin,
    mut wait_for_login: WaitForLogin,
) -> Result<(), String>
where
    ShowLogin: FnMut() -> Result<(), String>,
    WaitForLogin: FnMut(u64) -> (LoginState, bool),
{
    show_login()?;
    let (login_state, timed_out) = wait_for_login(timeout_seconds);
    if login_state == LoginState::LoggedIn {
        return Ok(());
    }

    let status = match login_state {
        LoginState::LoggedOut => "login still appears incomplete",
        LoginState::Unknown => "login status is still unknown",
        LoginState::LoggedIn => unreachable!("logged-in state returned above"),
    };
    let timing = if timed_out { "Timed out" } else { "Stopped" };
    Err(format!(
        "{} after {} seconds waiting for {} login; {}. Complete sign-in in the Chrome window, then retry.",
        timing,
        timeout_seconds.max(1),
        provider.display_name(),
        status
    ))
}

fn complete_query_login(
    config_path: &str,
    provider: Provider,
    timeout_seconds: u64,
    verbose: bool,
) -> Result<(), String> {
    println!(
        "\n{} is not logged in. Opening Chrome for sign-in...",
        provider.display_name()
    );
    println!(
        "Complete sign-in in Chrome. Your original query will continue automatically afterward.\n"
    );

    complete_query_login_with(
        provider,
        timeout_seconds,
        || {
            start_chrome_if_needed(false, verbose)?;
            let headful_config_path = write_mcp_config(!verbose, false)?;
            ensure_provider_tab(&headful_config_path, provider, false, false, verbose)
        },
        |timeout| wait_for_login_completion(config_path, provider, timeout, verbose),
    )?;

    println!(
        "Success: {} login detected. Continuing the original query...",
        provider.display_name()
    );
    Ok(())
}

fn print_chrome_diagnostics(profile_path: &str) {
    let snapshot = inspect_chrome_debug_port(profile_path);
    let recorded_pid = read_chrome_pid().unwrap_or_else(|| "unknown".to_string());

    println!("Chrome diagnostics:");
    println!("  profile: {}", profile_path);
    println!("  recorded PID: {}", recorded_pid);
    println!("  listener PIDs: {:?}", snapshot.listener_pids);
    println!("  ask-bridge owner PIDs: {:?}", snapshot.ask_pids);
    println!(
        "  CDP browser identity recorded: {}",
        snapshot
            .record
            .and_then(|record| record.browser_id)
            .is_some()
    );
}

/// How long to wait for a non-tty stdin to produce its first byte (or EOF)
/// when a prompt argument was already provided. Agent harnesses (Claude Code,
/// Codex) run commands with a pipe they may never close; blocking on EOF hung
/// whole runs (2026-07-11).
const STDIN_PIPE_GRACE: Duration = Duration::from_secs(2);

enum StdinProbe {
    Data,
    Eof,
}

/// Read stdin on a helper thread, signalling the first byte (or EOF) on one
/// channel and the full content on another, so the caller can bound how long
/// it waits for a pipe that might never deliver anything.
fn spawn_stdin_reader() -> (
    std::sync::mpsc::Receiver<StdinProbe>,
    std::sync::mpsc::Receiver<std::io::Result<String>>,
) {
    let (probe_tx, probe_rx) = std::sync::mpsc::channel();
    let (data_tx, data_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut first = [0u8; 1];
        match stdin.read(&mut first) {
            Ok(0) => {
                let _ = probe_tx.send(StdinProbe::Eof);
                let _ = data_tx.send(Ok(String::new()));
            }
            Ok(_) => {
                let _ = probe_tx.send(StdinProbe::Data);
                let mut bytes = vec![first[0]];
                let result = stdin.read_to_end(&mut bytes).and_then(|_| {
                    String::from_utf8(bytes)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                });
                let _ = data_tx.send(result);
            }
            Err(e) => {
                let _ = probe_tx.send(StdinProbe::Eof);
                let _ = data_tx.send(Err(e));
            }
        }
    });
    (probe_rx, data_rx)
}

/// With a prompt argument in hand piped stdin is an optional supplement: wait
/// up to `grace` for the pipe's first byte, then read a live pipe to EOF as
/// before; a silent pipe (agent harness holding it open) is treated as "no
/// piped input". Without a prompt argument stdin IS the prompt, so wait
/// unbounded exactly like upstream.
fn recv_piped_stdin(
    probe_rx: &std::sync::mpsc::Receiver<StdinProbe>,
    data_rx: &std::sync::mpsc::Receiver<std::io::Result<String>>,
    grace: Duration,
    has_prompt_argument: bool,
) -> std::io::Result<String> {
    if !has_prompt_argument {
        // stdin IS the prompt: wait unbounded like upstream, but after the
        // grace window tell the user what we are blocked on (an agent harness
        // holding the pipe open would otherwise hang here with no diagnostic).
        return match data_rx.recv_timeout(grace) {
            Ok(result) => result,
            Err(_) => {
                eprintln!(
                    "Waiting for a prompt on stdin (pipe is open; close it or pass a prompt argument)..."
                );
                data_rx.recv().unwrap_or(Ok(String::new()))
            }
        };
    }
    match probe_rx.recv_timeout(grace) {
        Ok(_) => data_rx.recv().unwrap_or(Ok(String::new())),
        Err(_) => {
            eprintln!(
                "No piped stdin data within {}s; continuing with the prompt argument only.",
                grace.as_secs()
            );
            Ok(String::new())
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = Cli::parse();
    if cli.command.is_none() {
        let is_stdin_terminal = io::stdin().is_terminal();
        if is_stdin_terminal && cli.prompt.as_deref() == Some("update") {
            cli.command = Some(Commands::Update);
        }
    }

    let command_verbose = match &cli.command {
        Some(Commands::Get { verbose, .. }) => cli.verbose || *verbose,
        _ => cli.verbose,
    };

    FORWARD_MCP_STDERR.store(command_verbose, std::sync::atomic::Ordering::Relaxed);

    if matches!(cli.command, Some(Commands::Config)) {
        if let Err(e) = run_config_command(cli.provider) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }

        return Ok(());
    }
    if matches!(cli.command, Some(Commands::Update)) {
        if let Err(e) = run_update_command() {
            eprintln!("Update failed: {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    let provider = match resolve_provider(cli.provider) {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = validate_provider_feature_support(provider, &cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    if !command_verbose {
        // SAFETY: Called before spawning other threads and before loading MCP config.
        unsafe {
            std::env::remove_var("MCP_DEBUG");
        }
    }
    if std::env::var("MCP_TIMEOUT").is_err() {
        // SAFETY: Called before spawning other threads and before loading MCP config.
        unsafe {
            std::env::set_var("MCP_TIMEOUT", "20");
        }
    }

    let is_terminal = io::stdout().is_terminal();
    let use_glow = is_terminal && is_glow_available();

    let is_headless = match &cli.command {
        Some(Commands::Login) => false, // Force headful only for login command so user can see it to log in
        Some(Commands::Get { .. }) => false, // Default get to headful for debugging by default
        _ => cli.headless, // Respect --headless (defaults to true) for all other commands (including Open)
    };

    if matches!(cli.command, Some(Commands::Close)) {
        let profile_path = match chrome_profile_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Error locating Chrome profile: {}", e);
                std::process::exit(1);
            }
        };

        match close_ask_chrome_on_debug_port(&profile_path) {
            Ok(true) => println!("Closed ask-bridge Chrome browser instance."),
            Ok(false) => println!("No ask-bridge Chrome browser instance is running."),
            Err(e) => {
                eprintln!("Error closing ask-bridge Chrome browser instance: {}", e);
                std::process::exit(1);
            }
        }

        return Ok(());
    }

    if let Err(e) = check_node_runtime() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    let config_path = match write_mcp_config(!command_verbose, is_headless) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = start_chrome_if_needed(is_headless, command_verbose) {
        eprintln!("Error starting Chrome: {}", e);
        std::process::exit(1);
    }

    if let Some(command) = cli.command {
        match command {
            Commands::Open { url } => {
                if let Some(url) = url {
                    let page_provider = Provider::from_url(&url).unwrap_or(provider);
                    if let Err(e) = open_url_tab(
                        &config_path,
                        page_provider,
                        &url,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error opening URL: {}", e);
                        std::process::exit(1);
                    }

                    match copy_latest_markdown(&config_path, page_provider) {
                        Ok(markdown) => {
                            if let Some(ref output_path) = cli.output {
                                let _ = std::fs::write(output_path, &markdown).map_err(|e| {
                                    eprintln!("Error writing output file: {}", e);
                                    std::process::exit(1);
                                });
                            }
                            if let Err(e) = render_markdown(&markdown, use_glow) {
                                eprintln!("Error rendering Markdown: {}", e);
                                std::process::exit(1);
                            }
                            if let Err(e) = download_images_from_latest_message(
                                &config_path,
                                page_provider,
                                cli.image_output.as_deref(),
                                command_verbose,
                            ) {
                                eprintln!("Error downloading images: {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error copying latest response Markdown: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    if let Err(e) = ensure_provider_tab(
                        &config_path,
                        provider,
                        false,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                        std::process::exit(1);
                    }
                    println!("Successfully opened {}!", provider.display_name());
                }
                return Ok(());
            }
            Commands::Get { url, .. } => {
                let mut page_provider = provider;
                if let Some(url) = url {
                    page_provider = Provider::from_url(&url).unwrap_or(provider);
                    if let Err(e) = open_url_tab(
                        &config_path,
                        page_provider,
                        &url,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error opening URL: {}", e);
                        std::process::exit(1);
                    }
                } else {
                    if let Err(e) = ensure_provider_tab(
                        &config_path,
                        provider,
                        false,
                        is_headless,
                        command_verbose,
                    ) {
                        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                        std::process::exit(1);
                    }
                }

                match copy_latest_markdown(&config_path, page_provider) {
                    Ok(markdown) => {
                        if let Some(ref output_path) = cli.output {
                            let _ = std::fs::write(output_path, &markdown).map_err(|e| {
                                eprintln!("Error writing output file: {}", e);
                                std::process::exit(1);
                            });
                        }
                        if let Err(e) = render_markdown(&markdown, use_glow) {
                            eprintln!("Error rendering Markdown: {}", e);
                            std::process::exit(1);
                        }
                        if let Err(e) = download_images_from_latest_message(
                            &config_path,
                            page_provider,
                            cli.image_output.as_deref(),
                            command_verbose,
                        ) {
                            eprintln!("Error downloading images: {}", e);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error copying latest response Markdown: {}", e);
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            Commands::Login => {
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                println!("\n========================================================");
                println!("Please complete the login manually in the Chrome window.");
                println!("The tool will automatically detect when login is complete every second.");
                println!("========================================================\n");

                let (login_state, timed_out) =
                    wait_for_login_completion(&config_path, provider, cli.timeout, command_verbose);

                match (login_state, timed_out) {
                    (LoginState::LoggedIn, _) => println!(
                        "Success: Logged in successfully! You can now use the `ask-bridge` command."
                    ),
                    (LoginState::LoggedOut, true) => println!(
                        "Warning: Login timeout reached ({} seconds). Login still appears incomplete.",
                        cli.timeout
                    ),
                    (LoginState::Unknown, true) => println!(
                        "Warning: Timeout reached ({} seconds). Login status is still unknown; please verify manually.",
                        cli.timeout
                    ),
                    (LoginState::LoggedOut, false) | (LoginState::Unknown, false) => println!(
                        "Warning: Login status changed while waiting. Please verify the result and rerun if needed."
                    ),
                }
                if command_verbose {
                    match chrome_profile_path() {
                        Ok(profile_path) => print_chrome_diagnostics(&profile_path),
                        Err(e) => eprintln!("Warning: Failed to locate Chrome profile: {}", e),
                    }
                }
                return Ok(());
            }
            Commands::Close => unreachable!("close command is handled before Chrome startup"),
            Commands::Config => unreachable!("config command is handled before Chrome startup"),
            Commands::Update => unreachable!("update command is handled before Chrome startup"),
            Commands::Dump => {
                let list_res = call_mcp_tool(&config_path, "list_pages", serde_json::json!({}))?;
                println!("All pages: {:?}", list_res);
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                let url_res = call_mcp_tool(
                    &config_path,
                    "evaluate_script",
                    serde_json::json!({
                        "function": "() => window.location.href"
                    }),
                )?;
                println!("Current page URL: {:?}", parse_script_result(&url_res));
                let res = call_mcp_tool(
                    &config_path,
                    "evaluate_script",
                    serde_json::json!({
                        "function": "() => document.body.innerHTML"
                    }),
                )?;
                let html = parse_script_result(&res)?
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                std::fs::create_dir_all("target").unwrap();
                std::fs::write("target/dump.html", html)?;
                println!("Dumped HTML to target/dump.html");
                return Ok(());
            }
            Commands::Screenshot => {
                if let Err(e) =
                    ensure_provider_tab(&config_path, provider, false, is_headless, command_verbose)
                {
                    eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
                    std::process::exit(1);
                }
                let res = call_mcp_tool(&config_path, "take_screenshot", serde_json::json!({}))?;

                let mut saved = false;
                if let Some(arr) = res.get("content").and_then(|c| c.as_array()) {
                    for item in arr {
                        if let Some(data) = item
                            .get("type")
                            .filter(|t| t.as_str() == Some("image"))
                            .and_then(|_| item.get("data"))
                            .and_then(|d| d.as_str())
                        {
                            use base64::{Engine as _, engine::general_purpose::STANDARD};
                            match STANDARD.decode(data.trim()) {
                                Ok(bytes) => {
                                    std::fs::create_dir_all("target").unwrap();
                                    std::fs::write("target/screenshot.png", bytes)?;
                                    println!("Saved screenshot to target/screenshot.png");
                                    saved = true;
                                    break;
                                }
                                Err(e) => {
                                    eprintln!("Failed to decode base64 image data: {}", e);
                                }
                            }
                        }
                    }
                }
                if !saved {
                    eprintln!(
                        "Could not find any image item in the tool response content. Full response: {:?}",
                        res
                    );
                }
                return Ok(());
            }
        }
    }

    // Read prompt from arguments and optionally append piped stdin content.
    let mut stdin_prompt = String::new();

    // Check if stdin is a pipe (not a tty)
    if !std::io::stdin().is_terminal() {
        let (probe_rx, data_rx) = spawn_stdin_reader();
        stdin_prompt =
            recv_piped_stdin(&probe_rx, &data_rx, STDIN_PIPE_GRACE, cli.prompt.is_some())?;
    }

    let prompt = match cli.prompt {
        Some(mut p) => {
            if !stdin_prompt.is_empty() {
                p.push_str("\n\n");
                p.push_str(&stdin_prompt);
            }
            p
        }
        None => stdin_prompt,
    };

    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        // No prompt and no command, print help
        let mut cmd = Cli::command();
        if let Some(version) = cmd.get_version() {
            println!("ask-bridge {}", version);
        } else {
            println!("ask-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        cmd.print_help()?;
        println!();
        std::process::exit(0);
    }

    if let Err(e) = ensure_provider_tab(
        &config_path,
        provider,
        cli.new,
        is_headless,
        command_verbose,
    ) {
        eprintln!("Error ensuring {} tab: {}", provider.display_name(), e);
        std::process::exit(1);
    }

    // Show attached images in the terminal before sending
    if !cli.images.is_empty() {
        for img_path in &cli.images {
            display_image_in_terminal(img_path);
        }
    }

    // Verify login
    match check_login_status(&config_path, provider, command_verbose) {
        Ok(LoginState::LoggedOut) => {
            if let Err(e) =
                complete_query_login(&config_path, provider, cli.timeout, command_verbose)
            {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Ok(LoginState::Unknown) => {
            eprintln!(
                "Warning: Could not confirm the {} account menu. Attempting to proceed...",
                provider.display_name()
            );
        }
        Ok(LoginState::LoggedIn) => {}
        Err(e) if command_verbose => {
            eprintln!(
                "Warning: Failed to verify login status: {}. Attempting to proceed...",
                e
            );
        }
        Err(_) => {}
    }

    // Switch model if requested (before uploading attachments / typing the prompt)
    if let Some(m) = &cli.model
        && let Err(e) = switch_model(&config_path, provider, m, command_verbose)
    {
        eprintln!("Error switching model: {}", e);
        std::process::exit(1);
    }

    // Upload any attached images/files before counting messages (so the UI is ready).
    // Copilot must be cleared first: clearing its React composer after an upload can
    // detach the attachment chips that the user just selected.
    let has_attachments = !cli.images.is_empty() || !cli.files.is_empty();
    let mut attachment_receipt = AttachmentReceipt::default();
    if provider == Provider::Copilot {
        if let Err(e) = ensure_copilot_attachment_baseline(&config_path) {
            eprintln!("Error checking Copilot attachment baseline: {}", e);
            std::process::exit(1);
        }
        if has_attachments {
            if let Err(e) = focus_and_clear_composer(&config_path, provider) {
                eprintln!("Error preparing Copilot composer for attachments: {}", e);
                std::process::exit(1);
            }
            if let Err(e) = ensure_copilot_attachment_baseline(&config_path) {
                eprintln!(
                    "Error re-checking Copilot attachments after clearing the composer: {}",
                    e
                );
                std::process::exit(1);
            }
        }
    }
    if has_attachments {
        attachment_receipt = match upload_attachments_to_provider(
            &config_path,
            provider,
            &cli.images,
            &cli.files,
            command_verbose,
        ) {
            Ok(receipt) => receipt,
            Err(e) => {
                eprintln!("Error attaching images/files: {}", e);
                std::process::exit(1);
            }
        };
    }

    // Get the initial response marker count before submitting the prompt. Copilot's
    // generated class names are unstable, so its localized response Copy actions
    // provide a more durable completion marker than a single message selector.
    let assistant_selector = serde_json::to_string(provider.assistant_selector())
        .map_err(|e| format!("Failed to serialize assistant selector: {}", e))?;
    let initial_count_js = r#"() => {
            if (!__IS_COPILOT__) {
                return document.querySelectorAll(__ASSISTANT_SELECTOR__).length;
            }
            const labelOf = (el) => [
                el.getAttribute('aria-label'),
                el.getAttribute('title'),
                el.getAttribute('data-testid'),
                el.textContent
            ].filter(Boolean).join(' ');
            return Array.from(document.querySelectorAll('button'))
                .filter((button) => {
                    const label = labelOf(button);
                    return /copy|複製|复制|コピー|복사/i.test(label) &&
                        !/code|程式碼|代码|table|表格/i.test(label) &&
                        !button.closest('pre, code, [class*=\"code\"], [data-testid*=\"code\"]');
                }).length;
        }"#
    .replace(
        "__IS_COPILOT__",
        if provider == Provider::Copilot {
            "true"
        } else {
            "false"
        },
    )
    .replace("__ASSISTANT_SELECTOR__", &assistant_selector);
    let count_res = call_mcp_tool(
        &config_path,
        "evaluate_script",
        serde_json::json!({
            "function": initial_count_js
        }),
    )?;
    let initial_response_count = parse_script_result(&count_res)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    if command_verbose {
        println!("Setting prompt text and submitting...");
    }
    let status = submit_prompt_to_provider(
        &config_path,
        provider,
        &prompt,
        &attachment_receipt,
        command_verbose,
    )
    .map_err(|e| format!("Text entry or submission failed: {}", e))?;

    if command_verbose {
        println!("Prompt submitted successfully: {}", status);
    }

    if command_verbose {
        println!("Waiting for {} response...", provider.display_name());
    }

    let mut finished = false;
    let mut wait_cycles = 0;
    let mut stable_done_checks = 0;
    let spinner_frames = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut spinner_idx = 0;

    let max_wait_cycles: usize =
        usize::try_from(cli.timeout.saturating_mul(10)).unwrap_or(usize::MAX);
    while !finished && wait_cycles < max_wait_cycles {
        // Max wait time: timeout seconds (timeout * 10 * 100ms)
        if is_terminal {
            let frame = spinner_frames[spinner_idx % spinner_frames.len()];
            print!(
                "\r\x1b[1;36m{}\x1b[0m 正在等待 {} 回應...",
                frame,
                provider.display_name()
            );
            io::stdout().flush()?;
            spinner_idx += 1;
        }

        if wait_cycles % 5 == 0 {
            let stop_selectors = provider.stop_button_selectors_json();
            let assistant_selector = serde_json::to_string(provider.assistant_selector())
                .map_err(|e| format!("Failed to serialize assistant selector: {}", e))?;
            let response_check_js = r#"() => {
                    const stopSelectors = __STOP_SELECTORS__;
                    const isVisible = (el) => {
                        if (!el || el.disabled || el.getAttribute('aria-disabled') === 'true') return false;
                        const style = window.getComputedStyle(el);
                        if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
                        const rect = el.getBoundingClientRect();
                        return rect.width > 0 && rect.height > 0;
                    };
                    const stopButton = stopSelectors.map((selector) => document.querySelector(selector)).find(isVisible);
                    const messages = document.querySelectorAll(__ASSISTANT_SELECTOR__);
                    const assistantIsNew = messages.length > __INITIAL_COUNT__;
                    const labelOf = (el) => [
                        el.getAttribute('aria-label'),
                        el.getAttribute('title'),
                        el.getAttribute('data-testid'),
                        el.textContent
                    ].filter(Boolean).join(' ');
                    const copilotResponseCount = Array.from(document.querySelectorAll('button'))
                        .filter((button) => {
                            const label = labelOf(button);
                            return /copy|複製|复制|コピー|복사/i.test(label) &&
                                !/code|程式碼|代码|table|表格/i.test(label) &&
                                !button.closest('pre, code, [class*=\"code\"], [data-testid*=\"code\"]');
                        }).length;
                    const copilotResponseIsNew = copilotResponseCount > __INITIAL_COUNT__;
                    const isNew = __IS_COPILOT__ ? copilotResponseIsNew : assistantIsNew;
                    
                    if (isVisible(stopButton)) {
                        if (__IS_COPILOT__) window.__ask_bridge_generation_seen = true;
                        return { status: "generating", isNew: isNew };
                    }

                    if (__IS_COPILOT__ && window.__ask_bridge_generation_seen) {
                        return { status: "done", isNew: true };
                    }
                    
                    if (isNew) {
                        return { status: "done", isNew: isNew };
                    }
                    
                    return { status: "waiting", isNew: isNew };
                }"#
            .replace("__STOP_SELECTORS__", stop_selectors)
            .replace("__ASSISTANT_SELECTOR__", &assistant_selector)
            .replace("__INITIAL_COUNT__", &initial_response_count.to_string())
            .replace(
                "__IS_COPILOT__",
                if provider == Provider::Copilot {
                    "true"
                } else {
                    "false"
                },
            );
            let check_res = match call_mcp_tool(
                &config_path,
                "evaluate_script",
                serde_json::json!({
                    "function": response_check_js
                }),
            ) {
                Ok(res) => res,
                Err(e) => {
                    if command_verbose {
                        eprintln!(
                            "Warning: Failed to poll {} response: {}",
                            provider.display_name(),
                            e
                        );
                    }
                    thread::sleep(Duration::from_millis(100));
                    wait_cycles += 1;
                    continue;
                }
            };

            if let Ok(parsed) = parse_script_result(&check_res) {
                let status = parsed["status"].as_str().unwrap_or("waiting");
                let is_new = parsed["isNew"].as_bool().unwrap_or(false);

                if status == "done" && is_new {
                    stable_done_checks += 1;
                    if stable_done_checks >= 3 {
                        finished = true;
                    }
                } else {
                    stable_done_checks = 0;
                }
            }
        }

        thread::sleep(Duration::from_millis(100));
        wait_cycles += 1;
    }

    if is_terminal {
        print!("\r\x1b[K");
        io::stdout().flush()?;
    }

    if !finished {
        return Err(format!(
            "{} response did not complete within the timeout period ({} seconds)",
            provider.display_name(),
            cli.timeout
        )
        .into());
    }

    if command_verbose {
        println!(
            "Copying final response from {} toolbar...",
            provider.display_name()
        );
    }
    let last_markdown = copy_latest_markdown(&config_path, provider).map_err(|e| {
        format!(
            "Failed to copy the completed response from {}: {}",
            provider.display_name(),
            e
        )
    })?;
    if last_markdown.trim().is_empty() {
        return Err(format!(
            "{} completed but returned an empty response",
            provider.display_name()
        )
        .into());
    }

    if let Err(e) = render_markdown(&last_markdown, use_glow) {
        eprintln!("Error rendering Markdown: {}", e);
    }

    if finished {
        let _ = download_images_from_latest_message(
            &config_path,
            provider,
            cli.image_output.as_deref(),
            command_verbose,
        )
        .map_err(|e| {
            eprintln!("Error downloading images: {}", e);
        });
    }

    // Print the URL link of the current conversation thread
    let url_opt = call_mcp_tool(
        &config_path,
        "evaluate_script",
        serde_json::json!({
            "function": "() => window.location.href"
        }),
    )
    .ok()
    .and_then(|url_val| parse_script_result(&url_val).ok())
    .and_then(|u| u.as_str().map(|s| s.to_string()));

    if let Some(url) = url_opt {
        if is_terminal {
            println!("\n🌐 \x1b[1mThread Link:\x1b[0m \x1b[4;36m{}\x1b[0m", url);
        } else {
            println!("\nThread Link: {}", url);
        }
    }

    if let Some(ref output_path) = cli.output {
        if let Err(e) = std::fs::write(output_path, &last_markdown) {
            eprintln!("Error writing output file: {}", e);
        } else if command_verbose {
            println!("Successfully wrote Markdown response to {}", output_path);
        }
    }

    Ok(())
}
