(() => {
  const protocol = 'a3s.activity.v1';
  const catalog = JSON.parse(document.getElementById('discipline-catalog').textContent);
  const form = document.getElementById('research-form');
  const disciplineOptions = document.getElementById('discipline-options');
  const subfieldOptions = document.getElementById('subfield-options');
  const evidenceOptions = document.getElementById('evidence-options');
  const error = document.getElementById('error');
  const hostStatus = document.getElementById('host-status');
  const projectNameInput = document.getElementById('project-name');
  const questionInput = document.getElementById('question');
  const scenarioInput = document.getElementById('scenario');
  const validationCriteriaInput = document.getElementById('validation-criteria');
  let activeDiscipline = catalog[0];
  let activeSubfield = activeDiscipline.subfields[0];

  function createButton(className, label, pressed, onClick) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = className;
    button.setAttribute('aria-pressed', String(pressed));
    button.addEventListener('click', onClick);
    const text = document.createElement('strong');
    text.textContent = label;
    button.appendChild(text);
    return button;
  }

  function renderDisciplines() {
    disciplineOptions.replaceChildren();
    catalog.forEach((discipline) => {
      const button = createButton(
        'discipline-card',
        discipline.label,
        discipline.id === activeDiscipline.id,
        () => {
          activeDiscipline = discipline;
          activeSubfield = discipline.subfields[0];
          renderDisciplines();
          renderSubfields();
          renderEvidenceOptions();
          updatePreview();
        }
      );
      const code = document.createElement('span');
      code.className = 'discipline-code';
      code.textContent = discipline.code;
      button.prepend(code);
      const description = document.createElement('small');
      description.textContent = discipline.description;
      button.appendChild(description);
      disciplineOptions.appendChild(button);
    });
  }

  function renderSubfields() {
    subfieldOptions.replaceChildren();
    activeDiscipline.subfields.forEach((subfield) => {
      const button = createButton('subfield-chip', subfield, subfield === activeSubfield, () => {
        activeSubfield = subfield;
        renderSubfields();
        updatePreview();
      });
      subfieldOptions.appendChild(button);
    });
  }

  function renderEvidenceOptions() {
    evidenceOptions.replaceChildren();
    activeDiscipline.sources.forEach((source) => {
      const label = document.createElement('label');
      label.className = 'evidence-option';
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.name = 'evidence-source';
      input.value = source.id;
      input.checked = source.selected;
      input.dataset.packageSkill = String(source.packageSkill);
      input.addEventListener('change', updatePreview);
      const text = document.createElement('span');
      text.textContent = source.label;
      label.append(input, text);
      if (source.packageSkill) {
        const badge = document.createElement('em');
        badge.textContent = '专业源';
        label.appendChild(badge);
      }
      evidenceOptions.appendChild(label);
    });
  }

  function selectedSources() {
    return Array.from(document.querySelectorAll('input[name="evidence-source"]:checked')).map((input) => {
      const source = activeDiscipline.sources.find((candidate) => candidate.id === input.value);
      return { ...source, packageSkill: input.dataset.packageSkill === 'true' };
    });
  }

  function selectedOptionText(select) {
    return select.options[select.selectedIndex].textContent.trim();
  }

  function updatePreview() {
    const sources = selectedSources();
    const usePackageSkill = sources.some((source) => source.packageSkill);
    document.getElementById('preview-project').textContent =
      projectNameInput.value.trim() || '等待命名科研项目';
    document.getElementById('preview-discipline').textContent = activeDiscipline.label;
    document.getElementById('preview-subfield').textContent = activeSubfield;
    document.getElementById('preview-scenario').textContent = selectedOptionText(scenarioInput);
    document.getElementById('preview-sources').textContent =
      sources.length > 0 ? sources.map((source) => source.label).join('、') : '尚未选择';
    document.getElementById('preview-question').textContent = questionInput.value.trim() || '等待输入科研问题';
    document.getElementById('preview-validation').textContent =
      validationCriteriaInput.value.trim() || '等待定义核验标准';
    const route = document.getElementById('capability-route');
    route.classList.toggle('package-skill', usePackageSkill);
    document.getElementById('capability-title').textContent = usePackageSkill
      ? 'Code + a3s-use-science 专业源'
      : 'Code 通用科研能力';
    document.getElementById('capability-detail').textContent = usePackageSkill
      ? '审核通过后附加宿主验证的同包 Skill，用于所选且由扩展明确支持的生物医学数据源。'
      : '仅交付审核后的结构化任务，使用 Code 当前可用的检索与分析能力。';
  }

  function dateRange() {
    const from = document.getElementById('date-from').value;
    const to = document.getElementById('date-to').value;
    if (from && to) return `${from} to ${to}`;
    if (from) return `from ${from}`;
    if (to) return `through ${to}`;
    return 'No date restriction supplied';
  }

  window.addEventListener('message', (event) => {
    if (event.source !== window.parent) return;
    const message = event.data;
    if (!message || message.protocol !== protocol || message.type !== 'host.init') return;
    const theme = message.payload && message.payload.theme;
    document.documentElement.dataset.theme = theme === 'dark' ? 'dark' : 'light';
    const packageId = message.payload && message.payload.packageId;
    hostStatus.textContent = packageId ? `已验证 · ${packageId}` : '宿主已验证';
  });

  form.addEventListener('input', updatePreview);
  form.addEventListener('change', updatePreview);
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    const projectName = projectNameInput.value.trim();
    const question = questionInput.value.trim();
    const validationCriteria = validationCriteriaInput.value.trim();
    const sources = selectedSources();
    if (!projectName || !question || !validationCriteria || sources.length === 0) {
      error.classList.add('visible');
      return;
    }
    error.classList.remove('visible');

    const usePackageSkill = sources.some((source) => source.packageSkill);
    const scenario = scenarioInput.value;
    const scenarioLabel = selectedOptionText(scenarioInput);
    const relatedFields = document.getElementById('related-fields').value.trim() || 'None supplied';
    const scope = document.getElementById('scope').value.trim() || 'No additional scope supplied';
    const exclusions = document.getElementById('exclusions').value.trim() || 'No explicit exclusions supplied';
    const language = document.getElementById('language').value;
    const output = document.getElementById('output').value;
    const outputLabel = selectedOptionText(document.getElementById('output'));
    const rigor = document.getElementById('rigor').value;
    const citationStyle = document.getElementById('citation-style').value;
    const sourceLabels = sources.map((source) => source.label);
    const packageSourceLabels = sources.filter((source) => source.packageSkill).map((source) => source.label);
    const routingInstruction = usePackageSkill
      ? `Use the verified a3s-use-science tools for these selected package-backed sources: ${packageSourceLabels.join(', ')}. Preserve PMID, DOI, ChEMBL, NCT, and Ensembl identifiers whenever present.`
      : 'Use only search and retrieval capabilities currently available in Code. Do not assume access to a named database or source that the host cannot reach.';
    const prompt = [
      'Conduct the following rigorous research task.',
      `Project name: ${projectName}.`,
      `Primary discipline: ${activeDiscipline.label}.`,
      `Subfield: ${activeSubfield}.`,
      `Research scenario: ${scenario}.`,
      `Related or interdisciplinary fields: ${relatedFields}.`,
      `Research question: ${question}`,
      `Evidence channels: ${sourceLabels.join(', ')}.`,
      `Date range: ${dateRange()}.`,
      `Evidence languages: ${language}.`,
      `Scope and constraints: ${scope}.`,
      `Exclusions: ${exclusions}.`,
      `Validation criteria: ${validationCriteria}.`,
      routingInstruction,
      `Apply a ${rigor} approach. Return ${output} using ${citationStyle}.`,
      'Work through explicit plan, execute, produce, preview, and review phases. Keep the resulting artifacts editable in Code or Work and identify the output files or structured artifacts created at each phase.',
      'Include a provenance note with every final artifact. The provenance note must record sources and stable identifiers, methods or code used, execution records, key parameters and environment assumptions, artifact relationships, and every verification item that remains incomplete.',
      'Separate peer-reviewed evidence, preprints, datasets, standards, primary sources, and secondary interpretation. Define inclusion and exclusion choices, triangulate important claims, preserve stable source identifiers, and state uncertainty, conflicting findings, inaccessible channels, and evidence gaps. Never invent source access, records, citations, or findings.',
    ].join('\n\n');

    window.parent.postMessage(
      {
        protocol,
        type: 'context.propose',
        payload: {
          title: '审核科研任务',
          summary: `${projectName} · ${activeDiscipline.label} · ${activeSubfield} · ${scenarioLabel}：${question}`,
          fields: [
            { label: '项目', value: projectName },
            { label: '学科', value: activeDiscipline.label },
            { label: '细分领域', value: activeSubfield },
            { label: '科研场景', value: scenarioLabel },
            { label: '证据渠道', value: sourceLabels.join('、') },
            { label: '时间范围', value: dateRange() },
            { label: '交叉领域', value: relatedFields },
            { label: '交付物', value: outputLabel },
            { label: '核验标准', value: validationCriteria },
          ],
          prompt,
          usePackageSkill,
        },
      },
      '*'
    );
  });

  renderDisciplines();
  renderSubfields();
  renderEvidenceOptions();
  updatePreview();
  window.parent.postMessage({ protocol, type: 'activity.ready' }, '*');
})();
