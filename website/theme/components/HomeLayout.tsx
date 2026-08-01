import { useState } from "react";
import { useLang, withBase } from "@rspress/core/runtime";
import { homeCopy, type Locale, type SurfaceKey } from "./home-copy";

const installCommand = "a3s install use --source release";
const surfaceOrder: SurfaceKey[] = ["tool", "mcp", "skill", "ui", "okf"];

function ArrowIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M3 8h9M8.5 3.5 13 8l-4.5 4.5" />
    </svg>
  );
}

function GitHubIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="M12 .8A11.5 11.5 0 0 0 8.36 23.2c.58.1.79-.25.79-.56v-2.2c-3.22.7-3.9-1.36-3.9-1.36-.52-1.34-1.28-1.7-1.28-1.7-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.2 1.77 1.2 1.03 1.78 2.71 1.27 3.37.97.1-.75.4-1.27.73-1.56-2.57-.3-5.28-1.3-5.28-5.7 0-1.27.45-2.3 1.19-3.11-.12-.3-.52-1.48.11-3.07 0 0 .97-.31 3.16 1.19a10.86 10.86 0 0 1 5.76 0c2.2-1.5 3.16-1.19 3.16-1.19.63 1.6.23 2.77.11 3.07.74.81 1.19 1.84 1.19 3.1 0 4.43-2.71 5.4-5.29 5.69.42.36.79 1.07.79 2.16v3.2c0 .31.21.67.8.55A11.5 11.5 0 0 0 12 .8Z" />
    </svg>
  );
}

function MarkdownHome({ locale }: { locale: Locale }) {
  const labels = homeCopy[locale];
  return (
    <main>
      <h1>A3S Use</h1>
      <p>{labels.subtitle}</p>
      <h2>{labels.modelTitle}</h2>
      <p>{labels.modelBody}</p>
      <ul>
        <li>Tool</li>
        <li>MCP</li>
        <li>Skill</li>
        <li>UI</li>
        <li>OKF</li>
      </ul>
      <h2>{labels.lifecycleTitle}</h2>
      <p>{labels.lifecycleBody}</p>
      <h2>{labels.trustTitle}</h2>
      <p>{labels.trustBody}</p>
    </main>
  );
}

export function HomeLayout() {
  const locale: Locale = useLang() === "zh" ? "zh" : "en";
  const labels = homeCopy[locale];
  const [selectedSurface, setSelectedSurface] = useState<SurfaceKey>("tool");
  const [copied, setCopied] = useState(false);
  const selected = labels.surfaces[selectedSurface];
  const localePrefix = locale === "en" ? "/en" : "";
  const route = (pathname: string) =>
    withBase(
      `${localePrefix}${pathname.startsWith("/") ? pathname : `/${pathname}`}`,
    );

  async function copyInstallCommand() {
    try {
      await navigator.clipboard.writeText(installCommand);
    } catch {
      const input = document.createElement("textarea");
      input.value = installCommand;
      input.style.position = "fixed";
      input.style.opacity = "0";
      document.body.appendChild(input);
      input.select();
      document.execCommand("copy");
      input.remove();
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }

  if (import.meta.env.SSG_MD) {
    return <MarkdownHome locale={locale} />;
  }

  return (
    <main className="a3s-use-home">
      <section className="use-hero">
        <div className="use-hero-copy">
          <div className="use-eyebrow">
            <span />
            {labels.eyebrow}
          </div>
          <h1>
            {labels.titleLead}
            <span>{labels.titleAccent}</span>
          </h1>
          <p className="use-hero-subtitle">{labels.subtitle}</p>
          <div className="use-actions">
            <a
              className="use-button use-button--primary"
              href={route("/guide/")}
            >
              {labels.getStarted}
              <ArrowIcon />
            </a>
            <a
              className="use-button use-button--secondary"
              href="https://github.com/A3S-Lab/Use"
            >
              <GitHubIcon />
              {labels.github}
            </a>
          </div>
          <div className="use-status-row" aria-label={labels.statusLabel}>
            <span className="use-status use-status--ready">
              <i />
              {labels.foundationLabel}
              <strong>{labels.available}</strong>
            </span>
            <span className="use-status use-status--building">
              <i />
              {labels.platformLabel}
              <strong>{labels.building}</strong>
            </span>
          </div>
          <div className="use-install">
            <div>
              <span>{labels.installLabel}</span>
              <small>{labels.installHint}</small>
            </div>
            <code>
              <span>$</span> {installCommand}
            </code>
            <button type="button" onClick={copyInstallCommand}>
              {copied ? labels.copied : labels.copy}
            </button>
          </div>
        </div>

        <div className="use-hero-visual" aria-label={labels.consoleLabel}>
          <div className="use-console-glow" />
          <div className="use-console">
            <header>
              <div className="use-console-dots" aria-hidden="true">
                <span />
                <span />
                <span />
              </div>
              <strong>{labels.consoleLabel}</strong>
              <span>schema v3</span>
            </header>
            <div className="use-console-command">
              <span>$</span> a3s-use capability snapshot --json
            </div>
            <dl>
              <div>
                <dt>{labels.consolePackage}</dt>
                <dd>acme/research@2.0.0</dd>
              </div>
              <div>
                <dt>{labels.consoleTarget}</dt>
                <dd>darwin-arm64</dd>
              </div>
              <div>
                <dt>{labels.consoleTrust}</dt>
                <dd className="is-green">TUF · SHA-256 verified</dd>
              </div>
              <div>
                <dt>{labels.consoleGeneration}</dt>
                <dd>0042 · 7f3a…e91c</dd>
              </div>
            </dl>
            <div className="use-console-surfaces">
              <div>
                <span>Tool</span>
                <code>convert</code>
                <i>READY</i>
              </div>
              <div>
                <span>MCP</span>
                <code>library</code>
                <i>READY</i>
              </div>
              <div>
                <span>Skill</span>
                <code>review</code>
                <i>READY</i>
              </div>
              <div>
                <span>UI</span>
                <code>review</code>
                <i>READY</i>
              </div>
              <div>
                <span>OKF</span>
                <code>domain-knowledge</code>
                <i>PENDING</i>
              </div>
            </div>
            <footer>
              <span className="use-pulse" />
              {labels.consoleReady}
            </footer>
          </div>
          <div className="use-float-card use-float-card--trust">
            <span>TRUST PATH</span>
            <strong>root → snapshot → target</strong>
          </div>
          <div className="use-float-card use-float-card--platform">
            <span>TARGETS</span>
            <strong>LINUX · MACOS · WINDOWS</strong>
          </div>
        </div>
      </section>

      <section className="use-section use-model" id="package-model">
        <header className="use-section-header">
          <div>
            <span className="use-section-eyebrow">{labels.modelEyebrow}</span>
            <h2>{labels.modelTitle}</h2>
          </div>
          <p>{labels.modelBody}</p>
        </header>
        <div className="use-plane-grid">
          <article className="use-plane use-plane--native">
            <span>{labels.nativePlane}</span>
            <h3>{labels.nativeTitle}</h3>
            <p>{labels.nativeBody}</p>
            <div className="use-native-art" aria-hidden="true">
              <div>bin/convert</div>
              <div>runtime/assets</div>
              <div>target/darwin-arm64</div>
            </div>
          </article>
          <article className="use-plane use-plane--cognitive">
            <span>{labels.cognitivePlane}</span>
            <h3>{labels.cognitiveTitle}</h3>
            <p>{labels.cognitiveBody}</p>
            <div className="use-cognitive-art" aria-hidden="true">
              {surfaceOrder.map((surface) => (
                <span key={surface}>{labels.surfaces[surface].label}</span>
              ))}
            </div>
          </article>
        </div>
        <div className="use-surface-explorer">
          <div className="use-surface-tabs" aria-label={labels.surfaceHint}>
            <p>{labels.surfaceHint}</p>
            {surfaceOrder.map((surface) => (
              <button
                aria-pressed={selectedSurface === surface}
                className={selectedSurface === surface ? "is-active" : ""}
                key={surface}
                onClick={() => setSelectedSurface(surface)}
                type="button"
              >
                <span>{labels.surfaces[surface].label}</span>
                <small>{labels.surfaces[surface].kind}</small>
              </button>
            ))}
          </div>
          <article className="use-surface-detail" key={selectedSurface}>
            <div>
              <span>{selected.label}</span>
              <code>{selected.kind}</code>
            </div>
            <h3>{selected.title}</h3>
            <p>{selected.body}</p>
            <ul>
              {selected.evidence.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          </article>
        </div>
      </section>

      <section className="use-section use-lifecycle" id="lifecycle">
        <header className="use-section-header">
          <div>
            <span className="use-section-eyebrow">
              {labels.lifecycleEyebrow}
            </span>
            <h2>{labels.lifecycleTitle}</h2>
          </div>
          <p>{labels.lifecycleBody}</p>
        </header>
        <div className="use-lifecycle-grid">
          {labels.lifecycle.map((step) => (
            <article key={step.number}>
              <span>{step.number}</span>
              <h3>{step.title}</h3>
              <p>{step.body}</p>
            </article>
          ))}
        </div>
      </section>

      <section className="use-section use-architecture" id="architecture">
        <div className="use-architecture-copy">
          <span className="use-section-eyebrow">
            {labels.architectureEyebrow}
          </span>
          <h2>{labels.architectureTitle}</h2>
          <p>{labels.architectureBody}</p>
          <a href={route("/guide/architecture.html")}>
            {labels.architectureLink}
            <ArrowIcon />
          </a>
        </div>
        <div className="use-architecture-flow">
          <div className="use-source-row">
            <span>LOCAL</span>
            <span>RELEASE</span>
            <span>TUF</span>
          </div>
          <div className="use-flow-line">
            <i />
          </div>
          <article className="use-flow-card use-flow-card--manager">
            <small>{labels.source}</small>
            <strong>{labels.manager}</strong>
            <span>{labels.managerBody}</span>
          </article>
          <div className="use-flow-line">
            <i />
          </div>
          <article className="use-flow-card use-flow-card--engine">
            <strong>{labels.engine}</strong>
            <span>{labels.engineBody}</span>
          </article>
          <div className="use-flow-branch">
            <i />
            <i />
          </div>
          <div className="use-flow-pair">
            <article className="use-flow-card">
              <strong>{labels.planes}</strong>
              <span>{labels.planesBody}</span>
            </article>
            <article className="use-flow-card">
              <strong>{labels.hosts}</strong>
              <span>{labels.hostsBody}</span>
            </article>
          </div>
        </div>
      </section>

      <section className="use-section use-trust" id="trust">
        <header className="use-section-header">
          <div>
            <span className="use-section-eyebrow">{labels.trustEyebrow}</span>
            <h2>{labels.trustTitle}</h2>
          </div>
          <p>{labels.trustBody}</p>
        </header>
        <div className="use-trust-grid">
          {labels.trustCards.map((card) => (
            <article key={card.tag}>
              <span>{card.tag}</span>
              <h3>{card.title}</h3>
              <p>{card.body}</p>
            </article>
          ))}
        </div>
      </section>

      <section className="use-section use-platforms" id="platforms">
        <div className="use-platform-copy">
          <span className="use-section-eyebrow">{labels.platformEyebrow}</span>
          <h2>{labels.platformTitle}</h2>
          <p>{labels.platformBody}</p>
        </div>
        <div className="use-platform-list">
          <div>
            <span className="use-os use-os--linux">L</span>
            <strong>Linux</strong>
            <small>{labels.supported}</small>
          </div>
          <div>
            <span className="use-os use-os--mac">M</span>
            <strong>macOS</strong>
            <small>{labels.supported}</small>
          </div>
          <div>
            <span className="use-os use-os--windows">W</span>
            <strong>Windows</strong>
            <small>{labels.preview}</small>
          </div>
        </div>
      </section>

      <section className="use-cta">
        <div>
          <span className="use-section-eyebrow">{labels.ctaEyebrow}</span>
          <h2>{labels.ctaTitle}</h2>
          <p>{labels.ctaBody}</p>
        </div>
        <div className="use-actions">
          <a className="use-button use-button--primary" href={route("/guide/")}>
            {labels.ctaPrimary}
            <ArrowIcon />
          </a>
          <a
            className="use-button use-button--secondary"
            href={route("/guide/roadmap.html")}
          >
            {labels.ctaSecondary}
          </a>
        </div>
      </section>

      <footer className="use-footer">
        <a href={route("/")}>A3S Use</a>
        <span>{labels.footer}</span>
        <a href="https://github.com/A3S-Lab/Use">GitHub ↗</a>
      </footer>
    </main>
  );
}
