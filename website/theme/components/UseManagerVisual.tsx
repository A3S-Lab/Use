import {
  Check,
  Cube,
  Fingerprint,
  FlowArrow,
  MagnifyingGlass,
  Package,
  ShieldCheck,
  SquaresFour,
  Stack,
  TreeStructure,
} from "@phosphor-icons/react";
import { withBase } from "@rspress/core/runtime";
import type { Locale } from "./home-copy";

const surfaces = ["Tool", "MCP", "OKF", "Flow", "Skill", "UI"];
const stepIcons = [TreeStructure, Fingerprint, ShieldCheck, FlowArrow];

export function UseManagerVisual({ locale }: { locale: Locale }) {
  const zh = locale === "zh";
  const steps = [
    {
      title: zh ? "解析精确包图" : "Resolve exact graph",
      detail: "a3s/science@0.3.0 · 4 packages",
      status: zh ? "已锁定" : "LOCKED",
    },
    {
      title: zh ? "验证来源与摘要" : "Verify source and digest",
      detail: "catalog-v3 · sha256:93b7…20e1",
      status: zh ? "已验证" : "VERIFIED",
    },
    {
      title: zh ? "审查权限与影响" : "Review grants and impact",
      detail: zh ? "6 个表面 · 2 项授权" : "6 surfaces · 2 grants",
      status: zh ? "已批准" : "APPROVED",
    },
    {
      title: zh ? "原子发布能力快照" : "Publish capability snapshot",
      detail: "generation 42 → 43",
      status: zh ? "已发布" : "PUBLISHED",
    },
  ];

  return (
    <figure
      aria-label={
        zh
          ? "A3S Use Manager 展示包图解析、来源验证、授权审查和能力快照原子发布"
          : "A3S Use Manager resolving a package graph, verifying provenance, reviewing grants, and publishing an atomic capability snapshot"
      }
      className="use-manager-scene use-motion-scene is-motion-active"
    >
      <div className="use-manager-window">
        <header className="use-manager-appbar">
          <span className="use-manager-brand">
            <img alt="" src={withBase("/a3s-use-mark.svg")} />
            <span>
              <strong>A3S Use</strong>
              <small>PACKAGE MANAGER</small>
            </span>
          </span>
          <span className="use-manager-search">
            <MagnifyingGlass aria-hidden="true" size={13} />
            {zh ? "搜索包和能力" : "Search packages and capabilities"}
            <kbd>⌘ K</kbd>
          </span>
          <em>
            <i /> {zh ? "开发预览" : "DEV PREVIEW"}
          </em>
        </header>

        <div className="use-manager-layout" aria-hidden="true">
          <aside className="use-manager-sidebar">
            <nav>
              <span className="is-active">
                <SquaresFour size={14} weight="duotone" />
                {zh ? "目录" : "Catalog"}
              </span>
              <span>
                <Package size={14} weight="duotone" />
                {zh ? "安装" : "Install"}
              </span>
              <span>
                <Stack size={14} weight="duotone" />
                {zh ? "能力" : "Capabilities"}
              </span>
            </nav>
            <div className="use-manager-scope">
              <small>{zh ? "当前作用域" : "CURRENT SCOPE"}</small>
              <strong>workspace/research</strong>
              <span>
                <i /> {zh ? "策略已加载" : "Policy loaded"}
              </span>
            </div>
          </aside>

          <section className="use-manager-main">
            <header className="use-package-heading">
              <span className="use-package-icon">
                <Cube size={19} weight="duotone" />
              </span>
              <span>
                <small>{zh ? "候选包图" : "CANDIDATE GRAPH"}</small>
                <strong>a3s/science@0.3.0</strong>
              </span>
              <em>{zh ? "待切换" : "PENDING CUTOVER"}</em>
            </header>

            <div className="use-manager-surfaces">
              <header>
                <strong>{zh ? "能力表面" : "Capability surfaces"}</strong>
                <span>6 / 6 {zh ? "已就绪" : "ready"}</span>
              </header>
              <ul>
                {surfaces.map((surface) => (
                  <li key={surface}>
                    <Check size={10} weight="bold" />
                    {surface}
                  </li>
                ))}
              </ul>
            </div>

            <ol className="use-manager-pipeline">
              <span className="use-manager-signal" />
              {steps.map((step, index) => {
                const Icon = stepIcons[index];
                return (
                  <li className={`is-step-${index + 1}`} key={step.title}>
                    <span>
                      <Icon size={14} weight="duotone" />
                    </span>
                    <div>
                      <strong>{step.title}</strong>
                      <small>{step.detail}</small>
                    </div>
                    <em>{step.status}</em>
                    <Check size={12} weight="bold" />
                  </li>
                );
              })}
            </ol>

            <footer className="use-manager-generation">
              <span>
                <ShieldCheck size={16} weight="duotone" />
                <span>
                  <strong>{zh ? "当前能力代际" : "Current generation"}</strong>
                  <small>snapshot/43 · graph sha256:7e52…c10a</small>
                </span>
              </span>
              <em>
                <i /> {zh ? "已生效" : "ACTIVE"}
              </em>
            </footer>
          </section>
        </div>
      </div>
    </figure>
  );
}
