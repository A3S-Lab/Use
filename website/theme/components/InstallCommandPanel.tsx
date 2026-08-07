import {
  AppleLogo,
  Check,
  Copy,
  LinuxLogo,
  TerminalWindow,
  WarningCircle,
  WindowsLogo,
} from "@phosphor-icons/react";
import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import type { HomeCopy, InstallerKey } from "./home-copy";

export const cliInstallCommands: Record<InstallerKey, string> = {
  unix: "curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.sh | sh",
  windows:
    "irm https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.ps1 | iex",
};

const installerOrder: InstallerKey[] = ["unix", "windows"];
type CopyState = "idle" | "copying" | "copied" | "error";
type CopyOutcome = Exclude<CopyState, "idle" | "copying">;

export function InstallCommandPanel({ labels }: { labels: HomeCopy }) {
  const [selectedInstaller, setSelectedInstaller] =
    useState<InstallerKey>("unix");
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const [lastCopyOutcome, setLastCopyOutcome] = useState<CopyOutcome | null>(
    null,
  );
  const copyResetTimer = useRef<number | undefined>(undefined);
  const tabButtons = useRef(new Map<InstallerKey, HTMLButtonElement>());
  const panelId = useId();
  const command = cliInstallCommands[selectedInstaller];
  const copyLabel =
    copyState === "copying"
      ? labels.copying
      : copyState === "copied"
        ? labels.copied
        : copyState === "error"
          ? labels.copyFailed
          : labels.copy;

  useEffect(
    () => () => {
      if (copyResetTimer.current !== undefined) {
        window.clearTimeout(copyResetTimer.current);
      }
    },
    [],
  );

  async function copyCommand() {
    setCopyState("copying");
    setLastCopyOutcome(null);
    let copySucceeded = false;

    if (navigator.clipboard?.writeText && window.isSecureContext) {
      let clipboardTimeout: number | undefined;

      try {
        await Promise.race([
          navigator.clipboard.writeText(command),
          new Promise<never>((_, reject) => {
            clipboardTimeout = window.setTimeout(
              () => reject(new Error("Clipboard write timed out.")),
              500,
            );
          }),
        ]);
        copySucceeded = true;
      } catch {
        copySucceeded = false;
      } finally {
        if (clipboardTimeout !== undefined) {
          window.clearTimeout(clipboardTimeout);
        }
      }
    }

    if (!copySucceeded) {
      const input = document.createElement("textarea");
      input.value = command;
      input.style.position = "fixed";
      input.style.opacity = "0";
      document.body.appendChild(input);
      input.select();
      copySucceeded = document.execCommand("copy");
      input.remove();
    }

    const outcome: CopyOutcome = copySucceeded ? "copied" : "error";
    setCopyState(outcome);
    setLastCopyOutcome(outcome);
    if (copyResetTimer.current !== undefined) {
      window.clearTimeout(copyResetTimer.current);
    }
    copyResetTimer.current = window.setTimeout(
      () => setCopyState("idle"),
      copySucceeded ? 1600 : 2600,
    );
  }

  function selectInstaller(nextInstaller: InstallerKey) {
    setSelectedInstaller(nextInstaller);
    setCopyState("idle");
    setLastCopyOutcome(null);
  }

  function handleTabKeyDown(
    event: KeyboardEvent<HTMLButtonElement>,
    currentInstaller: InstallerKey,
  ) {
    const currentIndex = installerOrder.indexOf(currentInstaller);
    let nextIndex: number | undefined;

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % installerOrder.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex =
        (currentIndex - 1 + installerOrder.length) % installerOrder.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = installerOrder.length - 1;
    }

    if (nextIndex === undefined) return;

    event.preventDefault();
    const nextInstaller = installerOrder[nextIndex];
    selectInstaller(nextInstaller);
    tabButtons.current.get(nextInstaller)?.focus();
  }

  return (
    <section className="use-section use-install-section" id="install">
      <header className="use-section-intro use-install-intro">
        <span>{labels.installer.kicker}</span>
        <h2>{labels.installer.title}</h2>
        <p>{labels.installer.body}</p>
      </header>

      <div className="use-install-layout">
        <div className="use-install-terminal">
          <div
            aria-label={labels.installer.platformSelector}
            className="use-install-tabs"
            role="tablist"
          >
            <button
              aria-controls={panelId}
              aria-selected={selectedInstaller === "unix"}
              onClick={() => selectInstaller("unix")}
              onKeyDown={(event) => handleTabKeyDown(event, "unix")}
              ref={(element) => {
                if (element) tabButtons.current.set("unix", element);
              }}
              role="tab"
              tabIndex={selectedInstaller === "unix" ? 0 : -1}
              type="button"
            >
              <AppleLogo aria-hidden="true" weight="fill" />
              <LinuxLogo aria-hidden="true" />
              {labels.installer.unix}
            </button>
            <button
              aria-controls={panelId}
              aria-selected={selectedInstaller === "windows"}
              onClick={() => selectInstaller("windows")}
              onKeyDown={(event) => handleTabKeyDown(event, "windows")}
              ref={(element) => {
                if (element) tabButtons.current.set("windows", element);
              }}
              role="tab"
              tabIndex={selectedInstaller === "windows" ? 0 : -1}
              type="button"
            >
              <WindowsLogo aria-hidden="true" weight="fill" />
              {labels.installer.windows}
            </button>
          </div>

          <div
            className="use-install-command"
            id={panelId}
            key={selectedInstaller}
            role="tabpanel"
            tabIndex={0}
          >
            <header>
              <span>
                <i />
                <i />
                <i />
              </span>
              <small>
                <TerminalWindow aria-hidden="true" />
                {selectedInstaller === "unix" ? "Terminal" : "PowerShell"}
              </small>
            </header>
            <pre>
              <span>{selectedInstaller === "unix" ? "$" : ">"}</span>
              <code>{command}</code>
            </pre>
            <button
              aria-label={copyLabel}
              className={`use-copy-button is-${copyState}`}
              data-install-copy
              disabled={copyState === "copying"}
              onClick={copyCommand}
              type="button"
            >
              {copyState === "copied" ? (
                <Check aria-hidden="true" weight="bold" />
              ) : copyState === "error" ? (
                <WarningCircle aria-hidden="true" weight="bold" />
              ) : (
                <Copy aria-hidden="true" />
              )}
              <span>{copyLabel}</span>
            </button>
            <span
              aria-atomic="true"
              aria-live="polite"
              className="use-visually-hidden"
              data-install-copy-status
              role="status"
            >
              {lastCopyOutcome === "copied"
                ? labels.copied
                : lastCopyOutcome === "error"
                  ? labels.copyFailed
                  : ""}
            </span>
          </div>

          <footer>
            <span>
              <Check aria-hidden="true" weight="bold" />
              SHA-256
            </span>
            <span>{labels.installer.atomicInstall}</span>
            <span>{labels.installer.noPathMutation}</span>
          </footer>
        </div>

        <aside className="use-install-next">
          <article>
            <span>01</span>
            <div>
              <h3>{labels.installer.cliTitle}</h3>
              <p>{labels.installer.cliBody}</p>
              <ul>
                <li>macOS x86_64 / arm64</li>
                <li>glibc Linux x86_64 / arm64</li>
                <li>Windows x86_64</li>
              </ul>
            </div>
          </article>
          <article className="is-preview">
            <span>02</span>
            <div>
              <h3>{labels.installer.previewTitle}</h3>
              <p>{labels.installer.previewBody}</p>
              <pre>
                <code>{`a3s install use --source release\na3s use doctor --json`}</code>
              </pre>
            </div>
          </article>
        </aside>
      </div>
    </section>
  );
}
