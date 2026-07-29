import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Database,
  Download,
  FileClock,
  Globe,
  KeyRound,
  Link,
  LoaderCircle,
  Maximize2,
  Monitor,
  Plus,
  RefreshCw,
  ShieldCheck,
  Sun,
  Moon,
  Terminal,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Language, formatResetTimeI18n, formatTimeI18n, t } from "./i18n";
import appIcon from "../src-tauri/icons/128x128.png";

type SwitchTarget = "desktop" | "ide" | "cli" | "win_cli" | "wsl_cli";
type AddAccountTab = "oauth" | "token" | "database";

interface Account {
  id: string;
  email: string;
  created_at: number;
  last_used_at: number;
  is_current: boolean;
  quota: QuotaData | null;
  last_target?: SwitchTarget | null;
}

type DatabaseImportOutcome = "added" | "updated" | "unchanged";

interface DatabaseImportResult {
  account: Account;
  outcome: DatabaseImportOutcome;
}

interface ModelQuota {
  name: string;
  percentage: number;
  reset_time: string;
  display_name: string | null;
}

interface QuotaData {
  models: ModelQuota[];
  last_updated: number;
  subscription_tier: string | null;
  is_forbidden: boolean;
}

interface AccountListResponse {
  accounts: Account[];
  current_target: SwitchTarget | null;
  target_accounts?: Record<string, string>;
}

interface ImportResult {
  imported: number;
  updated: number;
}

const targetLabels: Record<string, string> = {
  desktop: "Antigravity",
  ide: "Antigravity IDE",
  cli: "Win CLI",
  win_cli: "Win CLI",
  wsl_cli: "WSL CLI",
};

function getTargetDescriptions(lang: Language): Record<string, string> {
  return {
    desktop: t(lang, "targetDesktopDesc"),
    ide: t(lang, "targetIdeDesc"),
    cli: t(lang, "targetCliDesc"),
    win_cli: t(lang, "targetCliDesc"),
    wsl_cli: "仅写入默认 WSL 发行版的当前 Linux 用户凭据",
  };
}

const targetIcons: Record<string, typeof Monitor> = {
  desktop: Monitor,
  ide: ChevronRight,
  cli: Terminal,
  win_cli: Terminal,
  wsl_cli: Terminal,
};

async function call<T>(command: string, payload?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, payload);
}

function quotaSummary(account: Account, lang: Language): string {
  if (!account.quota) return t(lang, "quotaUnqueried");
  if (account.quota.is_forbidden) return t(lang, "quotaForbidden");
  if (!account.quota.models.length) return t(lang, "quotaNone");
  const average = Math.round(
    account.quota.models.reduce((total, model) => total + model.percentage, 0) / account.quota.models.length,
  );
  return t(lang, "quotaAvg", { avg: average });
}

function extractRefreshTokens(value: string): string[] {
  const input = value.trim();
  if (!input) return [];
  const tokens = new Set<string>();
  try {
    const parsed: unknown = JSON.parse(input);
    if (Array.isArray(parsed)) {
      for (const item of parsed) {
        if (!item || typeof item !== "object") continue;
        const candidate = item as { refresh_token?: unknown; token?: { refresh_token?: unknown } };
        const token = typeof candidate.refresh_token === "string"
          ? candidate.refresh_token.trim()
          : typeof candidate.token?.refresh_token === "string"
            ? candidate.token.refresh_token.trim()
            : "";
        if (token) tokens.add(token);
      }
    }
  } catch {
    // 不是 JSON 时会继续从普通文本中提取 Token。
  }
  for (const token of input.match(/1\/\/[A-Za-z0-9_-]+/g) || []) tokens.add(token);
  if (!tokens.size && !/\s/.test(input)) tokens.add(input);
  return [...tokens];
}

type AccentColor = "teal" | "violet" | "cyan" | "amber";

function App() {
  const [lang, setLang] = useState<Language>(() => (localStorage.getItem("agy_lang") as Language) || "zh-CN");
  const [theme, setTheme] = useState<"dark" | "light">(() => (localStorage.getItem("agy_theme") as "dark" | "light") || "dark");
  const [accent, setAccent] = useState<AccentColor>(() => (localStorage.getItem("agy_accent") as AccentColor) || "teal");
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [currentTarget, setCurrentTarget] = useState<SwitchTarget | null>(null);
  const [appVersion, setAppVersion] = useState<string | null>(null);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("agy_theme", theme);
  }, [theme]);

  useEffect(() => {
    document.documentElement.setAttribute("data-accent", accent);
    localStorage.setItem("agy_accent", accent);
  }, [accent]);

  useEffect(() => {
    let active = true;
    void getVersion()
      .then((version) => {
        if (active) setAppVersion(version);
      })
      .catch(() => {
        // 浏览器预览环境没有 Tauri 运行时，保留空白即可。
      });
    return () => {
      active = false;
    };
  }, []);

  const toggleTheme = () => {
    setTheme((prev) => (prev === "dark" ? "light" : "dark"));
  };
  const [targetAccounts, setTargetAccounts] = useState<Partial<Record<SwitchTarget, string>>>({});
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ type: "success" | "error"; text: string } | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [addTab, setAddTab] = useState<AddAccountTab>("oauth");
  const [refreshToken, setRefreshToken] = useState("");
  const [oauthUrl, setOauthUrl] = useState("");
  const [manualOAuthCode, setManualOAuthCode] = useState("");
  const [addMessage, setAddMessage] = useState<string | null>(null);
  const [showQuotaModal, setShowQuotaModal] = useState(false);
  const oauthCompletionInFlight = useRef(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const result = await call<AccountListResponse>("list_accounts");
      setAccounts(result.accounts);
      setCurrentTarget(result.current_target);
      setTargetAccounts(result.target_accounts ?? {});
      setSelectedAccountId((previous) => {
        if (previous && result.accounts.some((account) => account.id === previous)) return previous;
        return result.accounts.find((account) => account.is_current)?.id ?? result.accounts[0]?.id ?? null;
      });
    } catch (error) {
      setNotice({ type: "error", text: `无法读取账号：${String(error)}` });
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  useEffect(() => {
    if (!showAdd || addTab !== "oauth" || oauthUrl) return;
    let cancelled = false;
    void call<string>("prepare_oauth_url")
      .then((url) => {
        if (!cancelled) setOauthUrl(url);
      })
      .catch((error) => {
        if (!cancelled) setAddMessage(`无法准备 OAuth 授权：${String(error)}`);
      });
    return () => {
      cancelled = true;
    };
  }, [addTab, oauthUrl, showAdd]);

  useEffect(() => {
    if (!showAdd || addTab !== "oauth") return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen("oauth-callback-received", () => {
      if (oauthUrl) void completeOAuth();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [addTab, oauthUrl, showAdd]);

  const selectedAccount = accounts.find((account) => account.id === selectedAccountId) ?? accounts[0] ?? null;
  const hasPendingAction = pending !== null;

  const closeAdd = () => {
    void call<void>("cancel_oauth_login").catch(() => undefined);
    setShowAdd(false);
    setOauthUrl("");
    setManualOAuthCode("");
    setRefreshToken("");
    setAddMessage(null);
  };

  const openAdd = () => {
    setAddTab("oauth");
    setAddMessage(null);
    setShowAdd(true);
  };

  const changeAddTab = (tab: AddAccountTab) => {
    if (addTab === "oauth" && tab !== "oauth") {
      void call<void>("cancel_oauth_login").catch(() => undefined);
      setOauthUrl("");
      setManualOAuthCode("");
    }
    setAddMessage(null);
    setAddTab(tab);
  };

  const handleTokenAdd = async () => {
    const tokens = extractRefreshTokens(refreshToken);
    if (!tokens.length) {
      setAddMessage("请粘贴有效的 refresh token，或包含 refresh_token 的 JSON 数组。");
      return;
    }
    setPending("add:token");
    let succeeded = 0;
    let failed = 0;
    try {
      for (const [index, token] of tokens.entries()) {
        try {
          await call<Account>("add_account", { email: "", refreshToken: token });
          succeeded += 1;
        } catch {
          failed += 1;
        }
        setAddMessage(`正在保存账号：${index + 1}/${tokens.length}`);
      }
      await refresh();
      if (succeeded) {
        setNotice({
          type: failed ? "error" : "success",
          text: `已保存 ${succeeded} 个账号${failed ? `，${failed} 个失败` : ""}。`,
        });
        if (!failed) closeAdd();
      } else {
        setAddMessage("所有 refresh token 都未能保存；请检查其是否有效或账号是否已存在。");
      }
    } finally {
      setPending(null);
    }
  };

  const startOAuth = async () => {
    setPending("add:oauth:start");
    setAddMessage("正在打开默认浏览器，请完成 Google 授权…");
    try {
      await call<void>("open_oauth_browser");
    } catch (error) {
      setAddMessage(`无法打开浏览器：${String(error)}`);
    } finally {
      setPending(null);
    }
  };

  const completeOAuth = async () => {
    if (oauthCompletionInFlight.current) return;
    oauthCompletionInFlight.current = true;
    setPending("add:oauth:complete");
    setAddMessage("正在保存已授权账号…");
    try {
      const account = await call<Account>("complete_oauth_login");
      setNotice({ type: "success", text: `已添加 ${account.email}。` });
      await refresh();
      closeAdd();
    } catch (error) {
      setAddMessage(`OAuth 授权失败：${String(error)}`);
    } finally {
      oauthCompletionInFlight.current = false;
      setPending(null);
    }
  };

  const submitOAuthCode = async () => {
    if (!manualOAuthCode.trim()) return;
    setPending("add:oauth:manual");
    try {
      await call<void>("submit_oauth_code", { codeOrCallbackUrl: manualOAuthCode.trim() });
      await completeOAuth();
    } catch (error) {
      setAddMessage(`提交授权码失败：${String(error)}`);
      setPending(null);
    }
  };

  const copyOAuthUrl = async () => {
    try {
      await navigator.clipboard.writeText(oauthUrl);
      setAddMessage("授权链接已复制。若浏览器没有自动打开，请粘贴到浏览器中完成登录。");
    } catch {
      setAddMessage("无法自动复制，请手动复制下方完整授权链接。");
    }
  };

  const importDatabase = async (
    command: "import_default_database" | "import_database_file",
    payload: Record<string, unknown>,
  ) => {
    setPending("add:database");
    setAddMessage(t(lang, "validatingAccounts"));
    try {
      const result = await call<DatabaseImportResult>(command, payload);
      const text = result.outcome === "added"
        ? t(lang, "importDbSuccessAdded", { email: result.account.email })
        : result.outcome === "updated"
          ? t(lang, "importDbSuccessUpdated", { email: result.account.email })
          : t(lang, "importDbSuccessUnchanged", { email: result.account.email });
      setNotice({ type: "success", text });
      await refresh();
      closeAdd();
    } catch (error) {
      setAddMessage(`数据库导入失败：${String(error)}`);
    } finally {
      setPending(null);
    }
  };

  const importV1Backups = async () => {
    setPending("add:v1");
    setAddMessage(t(lang, "scanningV1"));
    try {
      const result = await call<ImportResult>("import_v1_accounts");
      await refresh();
      setNotice({ type: "success", text: t(lang, "importV1Success", { imported: result.imported, updated: result.updated }) });
      closeAdd();
    } catch (error) {
      setAddMessage(`V1 备份导入失败：${String(error)}`);
    } finally {
      setPending(null);
    }
  };

  const chooseDatabaseFile = async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: "Antigravity 数据库", extensions: ["vscdb"] }],
      });
      if (typeof selected === "string") await importDatabase("import_database_file", { path: selected });
    } catch (error) {
      setAddMessage(`无法选择数据库文件：${String(error)}`);
    }
  };

  const exportAccounts = async () => {
    setPending("backup:export");
    try {
      const selected = await saveDialog({
        defaultPath: "agy-switch.accounts.v1.json",
        filters: [{ name: "Agy Switch 账号备份", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      const savedPath = await call<string>("export_accounts_to_file", { path: selected });
      setNotice({ type: "success", text: `账号备份已保存到：${savedPath}` });
    } catch (error) {
      setNotice({ type: "error", text: `导出账号备份失败：${String(error)}` });
    } finally {
      setPending(null);
    }
  };

  const importAgyBackup = async () => {
    try {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: "Agy Switch 账号备份", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      setPending("add:backup");
      setAddMessage("正在验证并导入 Agy Switch 账号备份…");
      const result = await call<ImportResult>("import_backup_file", { path: selected });
      await refresh();
      setNotice({ type: "success", text: `账号备份导入完成：新增 ${result.imported} 个，更新 ${result.updated} 个。` });
      closeAdd();
    } catch (error) {
      setAddMessage(`账号备份导入失败：${String(error)}`);
    } finally {
      setPending(null);
    }
  };

  const switchAccount = async (account: Account, target: SwitchTarget) => {
    const key = `switch:${account.id}:${target}`;
    setPending(key);
    try {
      const message = await call<string>("switch_account", { accountId: account.id, target });
      setNotice({ type: "success", text: message });
      await refresh();
    } catch (error) {
      setNotice({ type: "error", text: `切换失败：${String(error)}` });
    } finally {
      setPending(null);
    }
  };

  const refreshQuota = async (account: Account) => {
    const key = `quota:${account.id}`;
    setPending(key);
    try {
      const updated = await call<Account>("fetch_account_quota", { accountId: account.id });
      const summary = updated.quota?.is_forbidden
        ? "该账号当前无权读取模型配额。"
        : `已刷新 ${updated.quota?.models.length ?? 0} 个模型的配额。`;
      setNotice({ type: "success", text: summary });
      await refresh();
    } catch (error) {
      setNotice({ type: "error", text: `刷新模型配额失败：${String(error)}` });
    } finally {
      setPending(null);
    }
  };

  const refreshAllQuotas = async () => {
    setPending("quota:all");
    try {
      const result = await call<{ refreshed: number; failed: number }>("refresh_all_quotas");
      setNotice({
        type: result.failed ? "error" : "success",
        text: t(lang, "refreshAllQuotasResult", { refreshed: result.refreshed, failed: result.failed }),
      });
      await refresh();
    } catch (error) {
      setNotice({ type: "error", text: t(lang, "refreshAllQuotasFailed", { error: String(error) }) });
    } finally {
      setPending(null);
    }
  };

  const deleteAccount = async (account: Account) => {
    if (!window.confirm(t(lang, "confirmDelete", { email: account.email }))) return;
    setPending(`delete:${account.id}`);
    try {
      await call<void>("delete_account", { accountId: account.id });
      setNotice({ type: "success", text: t(lang, "deleteSuccess") });
      await refresh();
    } catch (error) {
      setNotice({ type: "error", text: t(lang, "deleteFailed", { error: String(error) }) });
    } finally {
      setPending(null);
    }
  };

  return (
    <main className="app-shell">
      <header className="app-header">
        <div className="brand-lockup">
          <img src={appIcon} alt="" className="brand-icon" />
          <div>
            <p className="eyebrow">{t(lang, "appSubtitle")}</p>
            <div className="brand-title-row">
              <h1>{t(lang, "appTitle")}</h1>
              {appVersion && <span className="app-version" title="应用版本">v{appVersion}</span>}
            </div>
          </div>
        </div>
        <div className="header-actions">
          <div className="language-selector">
            <Globe size={16} />
            <select
              value={lang}
              onChange={(e) => {
                const nextLang = e.target.value as Language;
                setLang(nextLang);
                localStorage.setItem("agy_lang", nextLang);
              }}
              aria-label="Language"
            >
              <option value="zh-CN">简体中文</option>
              <option value="zh-TW">繁體中文</option>
              <option value="en-US">English</option>
            </select>
          </div>
          <button
            type="button"
            className="quiet-icon-button"
            onClick={toggleTheme}
            aria-label={theme === "dark" ? t(lang, "lightMode") : t(lang, "darkMode")}
            aria-pressed={theme === "dark"}
            title={theme === "dark" ? t(lang, "lightMode") : t(lang, "darkMode")}
          >
            {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
          </button>
          <div className="accent-picker" title="主题颜色">
            {(["teal", "violet", "cyan", "amber"] as AccentColor[]).map((c) => (
              <button
                key={c}
                type="button"
                className={`accent-dot ${c}${accent === c ? " selected" : ""}`}
                onClick={() => setAccent(c)}
                title={c === "teal" ? t(lang, "accentTeal") : c === "violet" ? t(lang, "accentViolet") : c === "cyan" ? t(lang, "accentCyan") : t(lang, "accentAmber")}
              />
            ))}
          </div>
          <button className="quiet-button" disabled={hasPendingAction} onClick={() => void exportAccounts()}>
            {pending === "backup:export" ? <LoaderCircle className="spin" size={16} /> : <Download size={16} />}
            {t(lang, "exportBackup")}
          </button>
          <button className="quiet-icon-button" disabled={loading || hasPendingAction} onClick={() => void refresh()} title={t(lang, "refreshAccounts")}>
            <RefreshCw size={18} className={loading ? "spin" : ""} />
          </button>
          <button className="primary-button" disabled={hasPendingAction} onClick={openAdd}>
            <Plus size={17} /> {t(lang, "addAccount")}
          </button>
        </div>
      </header>

      {notice && (
        <div className={`notice ${notice.type}`} role="status">
          <span>{notice.text}</span>
          <button type="button" onClick={() => setNotice(null)} title={t(lang, "close")}><X size={16} /></button>
        </div>
      )}

      <section className="switch-hero" aria-label={t(lang, "quickSwitch")}>
        <div className="switch-intro">
          <p className="eyebrow">{t(lang, "quickSwitch")}</p>
          <h2>{selectedAccount ? selectedAccount.email : t(lang, "noSavedAccounts")}</h2>
          <p className="hero-copy">
            {selectedAccount
              ? t(lang, "quickSwitchDesc")
              : t(lang, "addAccountHint")}
          </p>
          {selectedAccount && (
            <p className="last-switch">
              <ShieldCheck size={15} />
              {selectedAccount.is_current && currentTarget
                ? t(lang, "lastSwitchedTo", { target: targetLabels[currentTarget] })
                : selectedAccount.last_used_at
                  ? t(lang, "lastSwitched", { time: formatTimeI18n(selectedAccount.last_used_at, lang) })
                  : t(lang, "neverSwitched")}
            </p>
          )}
        </div>

        <div className="target-switcher" aria-label={t(lang, "switchTarget")}>
          {(["desktop", "ide", "win_cli", "wsl_cli"] as SwitchTarget[]).map((target) => {
            const TargetIcon = targetIcons[target] || Terminal;
            const key = selectedAccount ? `switch:${selectedAccount.id}:${target}` : "";
            const isRecordedTarget = Boolean(selectedAccount?.is_current && currentTarget === target);
            const descriptions = getTargetDescriptions(lang);
            return (
              <button
                type="button"
                className={`target-button${isRecordedTarget ? " is-recorded" : ""}`}
                key={target}
                disabled={!selectedAccount || hasPendingAction}
                onClick={() => selectedAccount && void switchAccount(selectedAccount, target)}
              >
                {pending === key ? <LoaderCircle className="spin" size={18} /> : <TargetIcon size={18} />}
                <span>
                  <strong>{targetLabels[target]}</strong>
                  <small>{isRecordedTarget ? t(lang, "lastSwitchedTo", { target: "" }).replace("：", "") : descriptions[target]}</small>
                </span>
              </button>
            );
          })}
        </div>
      </section>

      {/* 各端当前作用账号一览看板 */}
      <section className="surface-overview-bar" aria-label="各端当前作用账号一览">
        <div className="surface-overview-title">
          <ShieldCheck size={14} />
          <span>各端当前作用账号一览</span>
        </div>
        <div className="surface-overview-items">
          {(["desktop", "ide", "win_cli", "wsl_cli"] as SwitchTarget[]).map((target) => {
            const TargetIcon = targetIcons[target] || Terminal;
            const activeEmail = targetAccounts[target] || (target === "win_cli" && targetAccounts["cli"]) || (currentTarget === target && selectedAccount ? selectedAccount.email : null);
            return (
              <div key={target} className={`surface-overview-chip ${activeEmail ? "active" : ""}`}>
                <div className="surface-chip-header">
                  <TargetIcon size={15} />
                  <span className="surface-name">{targetLabels[target]}</span>
                </div>
                <div className="surface-email" title={activeEmail || "未切换"}>{activeEmail || "未切换"}</div>
              </div>
            );
          })}
        </div>
      </section>

      <div className="workspace-grid">
        <section className="account-workspace" aria-labelledby="accounts-heading">
          <div className="section-heading">
            <div>
              <p className="eyebrow">{t(lang, "localAccounts")}</p>
              <h2 id="accounts-heading">{t(lang, "selectAccountToSwitch")} <span>{accounts.length}</span></h2>
            </div>
            <button className="quiet-button" disabled={!accounts.length || hasPendingAction} onClick={() => void refreshAllQuotas()}>
              {pending === "quota:all" ? <LoaderCircle className="spin" size={16} /> : <RefreshCw size={16} />}
              {t(lang, "refreshAllQuotas")}
            </button>
          </div>

          {loading ? (
            <div className="empty-state"><LoaderCircle className="spin" size={22} /> {t(lang, "validatingAccounts")}</div>
          ) : accounts.length === 0 ? (
            <div className="empty-state">
              <KeyRound size={24} />
              <p>{t(lang, "noSavedAccounts")}</p>
              <span>{t(lang, "addAccountHint")}</span>
              <button className="primary-button" onClick={openAdd}><Plus size={16} /> {t(lang, "addAccount")}</button>
            </div>
          ) : (
            <div className="account-ledger">
              {accounts.map((account) => {
                const selected = account.id === selectedAccount?.id;
                return (
                  <article className={`account-row${selected ? " selected" : ""}`} key={account.id}>
                    <button
                      type="button"
                      className="account-select"
                      aria-pressed={selected}
                      onClick={() => setSelectedAccountId(account.id)}
                    >
                      <span className="account-avatar">{account.email.slice(0, 1).toUpperCase()}</span>
                      <span className="account-ident">
                        <strong>{account.email}</strong>
                        <small>{account.last_used_at ? t(lang, "lastSwitched", { time: formatTimeI18n(account.last_used_at, lang) }) : t(lang, "neverSwitched")}</small>
                      </span>
                      <span className="account-meta">
                        <small>{t(lang, "modelQuotaTitle")}</small>
                        <strong>{quotaSummary(account, lang)}</strong>
                      </span>
                      {account.is_current && <span className="current-badge">{t(lang, "lastUsed")}</span>}
                      <ChevronRight className="row-chevron" size={18} />
                    </button>
                    <button
                      type="button"
                      className="delete-button"
                      disabled={hasPendingAction}
                      onClick={() => void deleteAccount(account)}
                      title={t(lang, "deleteAccount", { email: account.email })}
                    >
                      {pending === `delete:${account.id}` ? <LoaderCircle className="spin" size={16} /> : <Trash2 size={16} />}
                    </button>
                  </article>
                );
              })}
            </div>
          )}
        </section>

        <aside className="quota-workspace" aria-label={t(lang, "quotaWorkspace")}>
          <div className="section-heading compact">
            <div>
              <p className="eyebrow">{t(lang, "modelQuotaHeader")}</p>
              <h2>{t(lang, "modelQuotaTitle")}</h2>
            </div>
            {selectedAccount && (
              <button className="quiet-icon-button" disabled={hasPendingAction} onClick={() => void refreshQuota(selectedAccount)} title={t(lang, "refreshQuotaTitle")}>
                {pending === `quota:${selectedAccount.id}` ? <LoaderCircle className="spin" size={16} /> : <RefreshCw size={16} />}
              </button>
            )}
          </div>

          {!selectedAccount ? (
            <p className="quota-empty">{t(lang, "quotaEmptyNoAccount")}</p>
          ) : !selectedAccount.quota ? (
            <div className="quota-empty"><p>{t(lang, "quotaEmptyNotQueried")}</p><span>{t(lang, "quotaEmptyClickRefresh")}</span></div>
          ) : selectedAccount.quota.is_forbidden ? (
            <div className="quota-empty"><CircleAlert size={18} /><p>{t(lang, "quotaEmptyForbidden")}</p></div>
          ) : selectedAccount.quota.models.length === 0 ? (
            <div className="quota-empty"><p>{t(lang, "quotaEmptyNoModels")}</p></div>
          ) : (() => {
            const allModels = selectedAccount.quota.models;
            let geminiCount = 0;
            let otherCount = 0;
            const visibleModels = allModels.filter((m) => {
              const label = m.display_name || m.name;
              if (/gemini/i.test(label)) {
                if (geminiCount < 1) {
                  geminiCount++;
                  return true;
                }
                return false;
              } else {
                if (otherCount < 1) {
                  otherCount++;
                  return true;
                }
                return false;
              }
            });
            const hasMoreModels = allModels.length > visibleModels.length;

            return (
              <>
                <p className="quota-context">
                  {selectedAccount.quota.subscription_tier ? t(lang, "subscription", { tier: selectedAccount.quota.subscription_tier }) + " · " : ""}
                  {t(lang, "updatedAt", { time: formatTimeI18n(selectedAccount.quota.last_updated, lang) })}
                </p>
                <div className="quota-list">
                  {visibleModels.map((model) => (
                    <div className="model-quota" key={model.name}>
                      <div className="model-quota-title">
                        <strong title={model.name}>{model.display_name || model.name}</strong>
                        <span>{model.percentage}%</span>
                      </div>
                      <div className="quota-track" aria-label={`${model.display_name || model.name} 剩余 ${model.percentage}%`}>
                        <i style={{ width: `${model.percentage}%` }} />
                      </div>
                      <small>{t(lang, "reset", { time: formatResetTimeI18n(model.reset_time, lang) })}</small>
                    </div>
                  ))}
                </div>
                {hasMoreModels && (
                  <button
                    className="quiet-button full-button show-more-button"
                    onClick={() => setShowQuotaModal(true)}
                  >
                    <Maximize2 size={14} /> {t(lang, "viewAllModels", { count: allModels.length })}
                  </button>
                )}
              </>
            );
          })()}
        </aside>
      </div>

      <aside className="safety-note">
        <ShieldCheck size={18} />
        <span><strong>{t(lang, "safetyNoteTitle")}</strong>{t(lang, "safetyNoteDesc")}</span>
      </aside>

      {showAdd && createPortal(
        <div
          className="modal-layer"
          role="presentation"
          onClick={(event) => {
            if (event.target === event.currentTarget) closeAdd();
          }}
        >
          <div className="add-modal" role="dialog" aria-modal="true" aria-label={t(lang, "addAccountModalHeading")}>
            <div className="modal-heading">
              <div>
                <p className="eyebrow">{t(lang, "addAccountModalTitle")}</p>
                <h2>{t(lang, "addAccountModalHeading")}</h2>
                <p>{t(lang, "addAccountModalDesc")}</p>
              </div>
              <button type="button" className="quiet-icon-button" onClick={closeAdd} title={t(lang, "close")}><X size={18} /></button>
            </div>

            <div className="add-tabs" role="tablist" aria-label={t(lang, "addAccountTabs")}>
              <button type="button" className={addTab === "oauth" ? "active" : ""} onClick={() => changeAddTab("oauth")}><Globe size={15} /> {t(lang, "tabOAuth")}</button>
              <button type="button" className={addTab === "token" ? "active" : ""} onClick={() => changeAddTab("token")}><KeyRound size={15} /> {t(lang, "tabToken")}</button>
              <button type="button" className={addTab === "database" ? "active" : ""} onClick={() => changeAddTab("database")}><Database size={15} /> {t(lang, "tabDatabase")}</button>
            </div>

            {addMessage && <p className="add-status">{addMessage}</p>}

            {addTab === "oauth" && (
              <section className="add-panel oauth-panel">
                <div className="oauth-icon"><Globe size={28} /></div>
                <h3>{t(lang, "oauthHeading")}</h3>
                <p>{t(lang, "oauthDesc")}</p>
                <button className="primary-button full-button" disabled={hasPendingAction} onClick={() => void startOAuth()}>
                  {pending === "add:oauth:start" ? <LoaderCircle className="spin" size={16} /> : <Globe size={16} />} {t(lang, "startOAuth")}
                </button>
                {oauthUrl && (
                  <>
                    <span className="field-caption">{t(lang, "oauthUrlHelp")}</span>
                    <button type="button" className="oauth-url" onClick={() => void copyOAuthUrl()} title={t(lang, "copyLink")}><Link size={14} /><code>{oauthUrl}</code></button>
                    <button className="quiet-button full-button" disabled={hasPendingAction} onClick={() => void completeOAuth()}>
                      {pending === "add:oauth:complete" ? <LoaderCircle className="spin" size={16} /> : <CheckCircle2 size={16} />} {t(lang, "authorizedContinue")}
                    </button>
                    <div className="manual-code">
                      <span className="field-caption">{t(lang, "manualCodeHelp")}</span>
                      <div>
                        <input value={manualOAuthCode} onChange={(event) => setManualOAuthCode(event.target.value)} placeholder={t(lang, "manualCodePlaceholder")} />
                        <button className="quiet-button" disabled={!manualOAuthCode.trim() || hasPendingAction} onClick={() => void submitOAuthCode()}>{t(lang, "submit")}</button>
                      </div>
                    </div>
                  </>
                )}
              </section>
            )}

            {addTab === "token" && (
              <section className="add-panel">
                <label>
                  {t(lang, "tokenLabel")}
                  <textarea value={refreshToken} onChange={(event) => setRefreshToken(event.target.value)} rows={7} placeholder={t(lang, "tokenPlaceholder")} />
                </label>
                <p className="security-note">{t(lang, "tokenHelp")}</p>
              </section>
            )}

            {addTab === "database" && (
              <section className="add-panel import-panel">
                <div className="import-scheme">
                  <h3><Database size={16} /> {t(lang, "dbImportHeading")}</h3>
                  <p>{t(lang, "dbImportDesc")}</p>
                </div>
                <div className="import-actions">
                  <button className="quiet-button" disabled={hasPendingAction} onClick={() => void importDatabase("import_default_database", {})}><CheckCircle2 size={16} /> {t(lang, "autoDetect")}</button>
                  <button className="quiet-button" disabled={hasPendingAction} onClick={() => void importDatabase("import_default_database", { target: "desktop" })}><Monitor size={16} /> Antigravity</button>
                  <button className="quiet-button" disabled={hasPendingAction} onClick={() => void importDatabase("import_default_database", { target: "ide" })}><ChevronRight size={16} /> Antigravity IDE</button>
                  <button className="quiet-button" disabled={hasPendingAction} onClick={() => void importDatabase("import_default_database", { target: "cli" })}><Terminal size={16} /> Antigravity CLI</button>
                  <button className="quiet-button" disabled={hasPendingAction} onClick={() => void chooseDatabaseFile()}><Database size={16} /> {t(lang, "chooseDbFile")}</button>
                </div>
                <div className="import-divider"><span>{t(lang, "restoreDivider")}</span></div>
                <div className="import-scheme">
                  <h3><Upload size={16} /> {t(lang, "agyBackupHeading")}</h3>
                  <p>{t(lang, "agyBackupDesc")}</p>
                </div>
                <button className="quiet-button full-button" disabled={hasPendingAction} onClick={() => void importAgyBackup()}>
                  {pending === "add:backup" ? <LoaderCircle className="spin" size={16} /> : <Upload size={16} />} {t(lang, "importAgyBackup")}
                </button>
                <div className="import-scheme v1-scheme">
                  <h3><FileClock size={16} /> {t(lang, "v1BackupHeading")}</h3>
                  <p>{t(lang, "v1BackupDesc")}</p>
                </div>
                <button className="quiet-button full-button" disabled={hasPendingAction} onClick={() => void importV1Backups()}>
                  {pending === "add:v1" ? <LoaderCircle className="spin" size={16} /> : <FileClock size={16} />} {t(lang, "importV1Backup")}
                </button>
              </section>
            )}

            <div className="modal-actions">
              <button type="button" className="quiet-button" disabled={pending === "add:oauth:complete"} onClick={closeAdd}>{t(lang, "cancel")}</button>
              {addTab === "token" && (
                <button type="button" className="primary-button" disabled={hasPendingAction || !refreshToken.trim()} onClick={() => void handleTokenAdd()}>
                  {pending === "add:token" && <LoaderCircle className="spin" size={16} />} {t(lang, "confirmAdd")}
                </button>
              )}
            </div>
          </div>
        </div>,
        document.body,
      )}

      {showQuotaModal && selectedAccount && selectedAccount.quota && createPortal(
        <div
          className="modal-layer"
          role="presentation"
          onClick={(event) => {
            if (event.target === event.currentTarget) setShowQuotaModal(false);
          }}
        >
          <div className="add-modal quota-modal" role="dialog" aria-modal="true" aria-label="全部模型配额">
            <div className="modal-heading">
              <div>
                <p className="eyebrow">MODEL QUOTA DETAILS</p>
                <h2>{selectedAccount.email} 的全部模型配额</h2>
                <p>
                  {selectedAccount.quota.subscription_tier ? t(lang, "subscription", { tier: selectedAccount.quota.subscription_tier }) + " · " : ""}
                  {t(lang, "updatedAt", { time: formatTimeI18n(selectedAccount.quota.last_updated, lang) })}
                </p>
              </div>
              <button
                className="quiet-icon-button"
                onClick={() => setShowQuotaModal(false)}
                title={t(lang, "close")}
              >
                <X size={18} />
              </button>
            </div>

            <div className="quota-modal-list">
              {selectedAccount.quota.models.map((model) => (
                <div className="model-quota" key={model.name}>
                  <div className="model-quota-title">
                    <strong title={model.name}>{model.display_name || model.name}</strong>
                    <span>{model.percentage}%</span>
                  </div>
                  <div className="quota-track" aria-label={`${model.display_name || model.name} 剩余 ${model.percentage}%`}>
                    <i style={{ width: `${model.percentage}%` }} />
                  </div>
                  <small>{t(lang, "reset", { time: formatResetTimeI18n(model.reset_time, lang) })}</small>
                </div>
              ))}
            </div>

            <div className="modal-actions">
              <button type="button" className="quiet-button" onClick={() => setShowQuotaModal(false)}>
                {t(lang, "close")}
              </button>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </main>
  );
}

export default App;
