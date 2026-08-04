import {
  AppleLogoIcon,
  ArrowRightIcon,
  ArrowUpRightIcon,
  CheckIcon,
  CopyIcon,
  GithubLogoIcon,
  LinuxLogoIcon,
  WarningCircleIcon,
  WindowsLogoIcon,
} from "@phosphor-icons/react";
import { useLang, withBase } from "@rspress/core/runtime";
import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { homeCopy, type Locale, type SurfaceKey } from "./home-copy";

const installCommand = "a3s install use --source release";
const surfaceOrder: SurfaceKey[] = [
  "tool",
  "mcp",
  "okf",
  "flow",
  "skill",
  "ui",
];
type CopyState = "idle" | "copying" | "copied" | "error";

function MarkdownHome({ locale }: { locale: Locale }) {
  const labels = homeCopy[locale];
  return (
    <main>
      <h1>A3S Use</h1>
      <p>{labels.subtitle}</p>
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
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const copyResetTimer = useRef<number | undefined>(undefined);
  const surfaceButtons = useRef(new Map<SurfaceKey, HTMLButtonElement>());
  const surfacePanelId = useId();
  const selected = labels.surfaces[selectedSurface];
  const copyLabel =
    copyState === "copying"
      ? labels.copying
      : copyState === "copied"
        ? labels.copied
        : copyState === "error"
          ? labels.copyFailed
          : labels.copy;
  const localePrefix = locale === "en" ? "/en" : "";
  const route = (pathname: string) =>
    withBase(
      `${localePrefix}${pathname.startsWith("/") ? pathname : `/${pathname}`}`,
    );
  const asset = (pathname: string) => withBase(pathname);

  useEffect(
    () => () => {
      if (copyResetTimer.current !== undefined) {
        window.clearTimeout(copyResetTimer.current);
      }
    },
    [],
  );

  async function copyInstallCommand() {
    setCopyState("copying");
    let copySucceeded = false;

    if (navigator.clipboard?.writeText && window.isSecureContext) {
      let clipboardTimeout: number | undefined;

      try {
        await Promise.race([
          navigator.clipboard.writeText(installCommand),
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
      input.value = installCommand;
      input.style.position = "fixed";
      input.style.opacity = "0";
      document.body.appendChild(input);
      input.select();
      copySucceeded = document.execCommand("copy");
      input.remove();
    }

    setCopyState(copySucceeded ? "copied" : "error");
    if (copyResetTimer.current !== undefined) {
      window.clearTimeout(copyResetTimer.current);
    }
    copyResetTimer.current = window.setTimeout(
      () => setCopyState("idle"),
      copySucceeded ? 1600 : 2600,
    );
  }

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

    if (nextIndex === undefined) {
      return;
    }

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
          <h1 id="use-hero-title">
            {labels.titleLead}
            <strong>{labels.titleAccent}</strong>
          </h1>
          <p className="use-hero-subtitle">{labels.subtitle}</p>
          <div className="use-actions">
            <a
              className="use-button use-button--primary"
              href={route("/guide/")}
            >
              {labels.getStarted}
              <ArrowRightIcon aria-hidden="true" weight="bold" />
            </a>
            <a
              className="use-button use-button--secondary"
              href="https://github.com/A3S-Lab/Use"
            >
              <GithubLogoIcon aria-hidden="true" weight="fill" />
              {labels.github}
            </a>
          </div>
        </div>

        <figure className="use-hero-media">
          <picture>
            <source
              media="(max-width: 880px)"
              srcSet={asset("/package-system-hero-mobile.avif")}
              type="image/avif"
            />
            <source
              srcSet={asset("/package-system-hero.avif")}
              type="image/avif"
            />
            <source
              media="(max-width: 880px)"
              srcSet={asset("/package-system-hero-mobile.jpg")}
              type="image/jpeg"
            />
            <img
              alt={labels.heroImageAlt}
              decoding="async"
              fetchPriority="high"
              height="1402"
              loading="eager"
              src={asset("/package-system-hero.jpg")}
              width="1122"
            />
          </picture>
        </figure>
      </section>

      <section className="use-release-bar" aria-label={labels.statusLabel}>
        <div className="use-install">
          <div className="use-install-copy">
            <strong>{labels.installLabel}</strong>
            <span>{labels.installHint}</span>
          </div>
          <code>{installCommand}</code>
          <button
            aria-label={copyLabel}
            className={`is-${copyState}`}
            disabled={copyState === "copying"}
            onClick={copyInstallCommand}
            type="button"
          >
            {copyState === "copied" ? (
              <CheckIcon aria-hidden="true" weight="bold" />
            ) : copyState === "error" ? (
              <WarningCircleIcon aria-hidden="true" weight="bold" />
            ) : (
              <CopyIcon aria-hidden="true" />
            )}
            <span aria-live="polite">{copyLabel}</span>
          </button>
        </div>
        <dl className="use-release-status">
          <div>
            <dt>{labels.foundationLabel}</dt>
            <dd>{labels.available}</dd>
          </div>
          <div>
            <dt>{labels.platformLabel}</dt>
            <dd>{labels.building}</dd>
          </div>
        </dl>
      </section>

      <section className="use-section use-model" id="package-model">
        <header className="use-section-intro">
          <h2>{labels.modelTitle}</h2>
          <p>{labels.modelBody}</p>
        </header>

        <div className="use-plane-grid">
          <article className="use-plane use-plane--native">
            <div>
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
              <h3>{labels.cognitiveTitle}</h3>
              <p>{labels.cognitiveBody}</p>
            </div>
            <ul className="use-surface-index" aria-label={labels.surfaceHint}>
              {surfaceOrder.map((surface) => (
                <li key={surface}>{labels.surfaces[surface].label}</li>
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
        <header className="use-section-intro use-section-intro--compact">
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
          <h2>{labels.architectureTitle}</h2>
          <p>{labels.architectureBody}</p>
          <a href={route("/guide/architecture.html")}>
            {labels.architectureLink}
            <ArrowRightIcon aria-hidden="true" weight="bold" />
          </a>
        </div>
        <div
          className="use-architecture-flow"
          aria-label={labels.architectureTitle}
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
        <header className="use-section-intro">
          <h2>{labels.trustTitle}</h2>
          <p>{labels.trustBody}</p>
        </header>
        <figure className="use-trust-media">
          <picture>
            <source
              srcSet={asset("/package-trust-detail.avif")}
              type="image/avif"
            />
            <img
              alt={labels.trustImageAlt}
              decoding="async"
              height="1024"
              loading="lazy"
              src={asset("/package-trust-detail.jpg")}
              width="1536"
            />
          </picture>
        </figure>
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
          <h2>{labels.platformTitle}</h2>
          <p>{labels.platformBody}</p>
        </div>
        <dl className="use-platform-list">
          <div>
            <dt>
              <LinuxLogoIcon aria-hidden="true" />
              Linux
            </dt>
            <dd>{labels.supported}</dd>
          </div>
          <div>
            <dt>
              <AppleLogoIcon aria-hidden="true" weight="fill" />
              macOS
            </dt>
            <dd>{labels.supported}</dd>
          </div>
          <div>
            <dt>
              <WindowsLogoIcon aria-hidden="true" weight="fill" />
              Windows
            </dt>
            <dd>{labels.preview}</dd>
          </div>
        </dl>
      </section>

      <section className="use-cta">
        <div>
          <h2>{labels.ctaTitle}</h2>
          <p>{labels.ctaBody}</p>
        </div>
        <div className="use-actions">
          <a className="use-button use-button--primary" href={route("/guide/")}>
            {labels.getStarted}
            <ArrowRightIcon aria-hidden="true" weight="bold" />
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
          <ArrowUpRightIcon aria-hidden="true" weight="bold" />
        </a>
      </footer>
    </main>
  );
}
