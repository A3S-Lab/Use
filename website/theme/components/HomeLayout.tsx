import {
  AppleLogo,
  ArrowRight,
  ArrowUpRight,
  ArrowsClockwise,
  Check,
  Fingerprint,
  GithubLogo,
  LinuxLogo,
  ShieldCheck,
  TreeStructure,
  WarningCircle,
  WindowsLogo,
} from "@phosphor-icons/react";
import { useLang, withBase } from "@rspress/core/runtime";
import { type KeyboardEvent, useId, useRef, useState } from "react";
import { cliInstallCommands, InstallCommandPanel } from "./InstallCommandPanel";
import { homeCopy, type Locale, type SurfaceKey } from "./home-copy";
import { UseManagerVisual } from "./UseManagerVisual";

const surfaceOrder: SurfaceKey[] = [
  "tool",
  "mcp",
  "okf",
  "flow",
  "skill",
  "ui",
];
const assuranceIcons = [TreeStructure, Fingerprint, ArrowsClockwise];

function MarkdownHome({ locale }: { locale: Locale }) {
  const labels = homeCopy[locale];
  return (
    <main>
      <h1>
        A3S Use — {labels.titleLead} {labels.titleAccent}
      </h1>
      <p>{labels.subtitle}</p>
      <p>{labels.previewNotice}</p>
      <h2>{labels.installer.title}</h2>
      <pre>{cliInstallCommands.unix}</pre>
      <pre>{cliInstallCommands.windows}</pre>
      <h2>{labels.modelTitle}</h2>
      <p>{labels.modelBody}</p>
      <ul>
        {surfaceOrder.map((surface) => (
          <li key={surface}>{labels.surfaces[surface].label}</li>
        ))}
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
  const surfaceButtons = useRef(new Map<SurfaceKey, HTMLButtonElement>());
  const surfacePanelId = useId();
  const selected = labels.surfaces[selectedSurface];
  const localePrefix = locale === "en" ? "/en" : "";
  const route = (pathname: string) =>
    withBase(
      `${localePrefix}${pathname.startsWith("/") ? pathname : `/${pathname}`}`,
    );

  function handleSurfaceKeyDown(
    event: KeyboardEvent<HTMLButtonElement>,
    currentSurface: SurfaceKey,
  ) {
    const currentIndex = surfaceOrder.indexOf(currentSurface);
    let nextIndex: number | undefined;

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % surfaceOrder.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex =
        (currentIndex - 1 + surfaceOrder.length) % surfaceOrder.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = surfaceOrder.length - 1;
    }

    if (nextIndex === undefined) return;

    event.preventDefault();
    const nextSurface = surfaceOrder[nextIndex];
    setSelectedSurface(nextSurface);
    surfaceButtons.current.get(nextSurface)?.focus();
  }

  if (import.meta.env.SSG_MD) {
    return <MarkdownHome locale={locale} />;
  }

  return (
    <main className="a3s-use-home">
      <section className="use-hero" aria-labelledby="use-hero-title">
        <div className="use-hero-copy">
          <span className="use-kicker">{labels.heroKicker}</span>
          <h1 id="use-hero-title">
            <span>A3S Use</span>
            {labels.titleLead} <strong>{labels.titleAccent}</strong>
          </h1>
          <p className="use-hero-subtitle">{labels.subtitle}</p>
          <p className="use-preview-notice">
            <WarningCircle aria-hidden="true" weight="duotone" />
            {labels.previewNotice}
          </p>
          <div className="use-actions">
            <a className="use-button use-button--primary" href="#package-model">
              {labels.exploreModel}
              <ArrowRight aria-hidden="true" weight="bold" />
            </a>
            <a
              className="use-button use-button--secondary"
              href="https://github.com/A3S-Lab/Use"
            >
              <GithubLogo aria-hidden="true" weight="fill" />
              {labels.github}
            </a>
          </div>
        </div>

        <div className="use-hero-visual">
          <UseManagerVisual locale={locale} />
        </div>
      </section>

      <aside aria-label={labels.assuranceLabel} className="use-assurance-bar">
        <strong>{labels.assuranceLabel}</strong>
        <ul>
          {labels.assurances.map((assurance, index) => {
            const Icon = assuranceIcons[index];
            return (
              <li key={assurance.title}>
                <b>
                  <Icon aria-hidden="true" weight="duotone" />
                </b>
                <span>
                  <strong>{assurance.title}</strong>
                  <small>{assurance.body}</small>
                </span>
              </li>
            );
          })}
        </ul>
      </aside>

      <InstallCommandPanel labels={labels} />

      <section className="use-section use-model" id="package-model">
        <header className="use-section-intro">
          <span>{labels.modelKicker}</span>
          <h2>{labels.modelTitle}</h2>
          <p>{labels.modelBody}</p>
        </header>

        <div className="use-plane-grid">
          <article className="use-plane use-plane--native">
            <div>
              <small>NATIVE PLANE</small>
              <h3>{labels.nativeTitle}</h3>
              <p>{labels.nativeBody}</p>
            </div>
            <dl className="use-native-manifest">
              <div>
                <dt>binary</dt>
                <dd>bin/convert</dd>
              </div>
              <div>
                <dt>assets</dt>
                <dd>runtime/assets</dd>
              </div>
              <div>
                <dt>target</dt>
                <dd>darwin-arm64</dd>
              </div>
            </dl>
          </article>
          <article className="use-plane use-plane--cognitive">
            <div>
              <small>COGNITIVE PLANE</small>
              <h3>{labels.cognitiveTitle}</h3>
              <p>{labels.cognitiveBody}</p>
            </div>
            <ul className="use-surface-index" aria-label={labels.surfaceHint}>
              {surfaceOrder.map((surface) => (
                <li key={surface}>
                  <Check aria-hidden="true" weight="bold" />
                  {labels.surfaces[surface].label}
                </li>
              ))}
            </ul>
          </article>
        </div>

        <div className="use-surface-explorer">
          <div
            aria-label={labels.surfaceHint}
            className="use-surface-tabs"
            role="tablist"
          >
            {surfaceOrder.map((surface) => {
              const surfaceCopy = labels.surfaces[surface];
              const isSelected = selectedSurface === surface;
              return (
                <button
                  aria-controls={surfacePanelId}
                  aria-selected={isSelected}
                  key={surface}
                  onClick={() => setSelectedSurface(surface)}
                  onKeyDown={(event) => handleSurfaceKeyDown(event, surface)}
                  ref={(element) => {
                    if (element) {
                      surfaceButtons.current.set(surface, element);
                    } else {
                      surfaceButtons.current.delete(surface);
                    }
                  }}
                  role="tab"
                  tabIndex={isSelected ? 0 : -1}
                  type="button"
                >
                  <span>{surfaceCopy.label}</span>
                  <small>{surfaceCopy.kind}</small>
                </button>
              );
            })}
          </div>
          <article
            className="use-surface-detail"
            id={surfacePanelId}
            key={selectedSurface}
            role="tabpanel"
            tabIndex={0}
          >
            <small>{selected.kind}</small>
            <h3>{selected.title}</h3>
            <p>{selected.body}</p>
            <ul>
              {selected.evidence.map((item) => (
                <li key={item}>
                  <Check aria-hidden="true" weight="bold" />
                  {item}
                </li>
              ))}
            </ul>
          </article>
        </div>
      </section>

      <section className="use-section use-lifecycle" id="lifecycle">
        <header className="use-section-intro use-section-intro--compact">
          <span>{labels.lifecycleKicker}</span>
          <h2>{labels.lifecycleTitle}</h2>
          <p>{labels.lifecycleBody}</p>
        </header>
        <ol className="use-lifecycle-track">
          {labels.lifecycle.map((step) => (
            <li key={step.number}>
              <span aria-hidden="true">{step.number}</span>
              <h3>{step.title}</h3>
              <p>{step.body}</p>
            </li>
          ))}
        </ol>
      </section>

      <section className="use-section use-architecture" id="architecture">
        <div className="use-architecture-copy">
          <span className="use-kicker">{labels.architectureKicker}</span>
          <h2>{labels.architectureTitle}</h2>
          <p>{labels.architectureBody}</p>
          <a href={route("/guide/architecture.html")}>
            {labels.architectureLink}
            <ArrowRight aria-hidden="true" weight="bold" />
          </a>
        </div>
        <div
          aria-label={labels.architectureTitle}
          className="use-architecture-flow"
        >
          <div className="use-source-row">
            <span>Local</span>
            <span>Release</span>
            <span>TUF</span>
          </div>
          <div className="use-flow-connector" aria-hidden="true" />
          <article className="use-flow-node use-flow-node--manager">
            <small>{labels.source}</small>
            <strong>{labels.manager}</strong>
            <span>{labels.managerBody}</span>
          </article>
          <div className="use-flow-connector" aria-hidden="true" />
          <article className="use-flow-node">
            <strong>{labels.engine}</strong>
            <span>{labels.engineBody}</span>
          </article>
          <div className="use-flow-branch" aria-hidden="true" />
          <div className="use-flow-pair">
            <article className="use-flow-node">
              <strong>{labels.planes}</strong>
              <span>{labels.planesBody}</span>
            </article>
            <article className="use-flow-node">
              <strong>{labels.hosts}</strong>
              <span>{labels.hostsBody}</span>
            </article>
          </div>
        </div>
      </section>

      <section className="use-section use-trust" id="trust">
        <div className="use-trust-layout">
          <header className="use-section-intro">
            <span>{labels.trustKicker}</span>
            <h2>{labels.trustTitle}</h2>
            <p>{labels.trustBody}</p>
          </header>
          <div className="use-trust-ledger" aria-label={labels.trustLedger}>
            <header>
              <span>
                <ShieldCheck aria-hidden="true" weight="duotone" />
                {labels.trustLedger}
              </span>
              <em>
                <i /> {labels.trustVerified}
              </em>
            </header>
            <dl>
              <div>
                <dt>catalog/root.json</dt>
                <dd>TUF · role 18</dd>
                <Check aria-hidden="true" weight="bold" />
              </div>
              <div>
                <dt>package.archive</dt>
                <dd>sha256:93b7…20e1</dd>
                <Check aria-hidden="true" weight="bold" />
              </div>
              <div>
                <dt>reviewed.plan</dt>
                <dd>sha256:51cf…9ab0</dd>
                <Check aria-hidden="true" weight="bold" />
              </div>
              <div>
                <dt>capability.snapshot</dt>
                <dd>generation/43</dd>
                <Check aria-hidden="true" weight="bold" />
              </div>
            </dl>
            <footer>
              <Fingerprint aria-hidden="true" weight="duotone" />
              <span>
                <strong>receipt-v3</strong>
                <small>scope + graph + grants + generation</small>
              </span>
            </footer>
          </div>
        </div>
        <ul className="use-trust-principles">
          {labels.trustCards.map((card) => (
            <li key={card.title}>
              <h3>{card.title}</h3>
              <p>{card.body}</p>
            </li>
          ))}
        </ul>
      </section>

      <section className="use-section use-platforms" id="platforms">
        <div className="use-platform-copy">
          <span className="use-kicker">{labels.platformKicker}</span>
          <h2>{labels.platformTitle}</h2>
          <p>{labels.platformBody}</p>
        </div>
        <dl className="use-platform-list">
          <div>
            <dt>
              <LinuxLogo aria-hidden="true" />
              Linux
              <small>x86_64 · arm64</small>
            </dt>
            <dd>{labels.fullGate}</dd>
          </div>
          <div>
            <dt>
              <AppleLogo aria-hidden="true" weight="fill" />
              macOS
              <small>x86_64 · arm64</small>
            </dt>
            <dd>{labels.fullGate}</dd>
          </div>
          <div>
            <dt>
              <WindowsLogo aria-hidden="true" weight="fill" />
              Windows
              <small>x86_64</small>
            </dt>
            <dd className="is-preview">{labels.previewGate}</dd>
          </div>
        </dl>
      </section>

      <section className="use-cta">
        <div>
          <span>{labels.ctaKicker}</span>
          <h2>{labels.ctaTitle}</h2>
          <p>{labels.ctaBody}</p>
        </div>
        <div className="use-actions">
          <a className="use-button use-button--primary" href="#install">
            {labels.installer.title}
            <ArrowRight aria-hidden="true" weight="bold" />
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
        <a href="https://github.com/A3S-Lab/Use">
          GitHub
          <ArrowUpRight aria-hidden="true" weight="bold" />
        </a>
      </footer>
    </main>
  );
}
