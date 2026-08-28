const $ = id => document.getElementById(id);
let catalog = [], current = null, selectedId = null, request = 0, analyzing = false, pending = null;
const snapshots = new Map();
const PROPERTY_HELP = {
  identity: 'The exact source declaration this record belongs to: module identity, lexical qualified name, definition kind, and UTF-8 byte range. Runtime construction must bind to the real matching object; a matching name alone is not authority.',
  bases: 'Base classes resolved from source, in declaration order. These are logical references, not proof of the actual runtime bases or a physical memory layout.',
  metaclass: 'What the checker knows about the metaclass used to construct this class. Unsupported or uncertain metaclass behavior can make the class dynamic; runtime namespace and construction checks remain necessary.',
  decorators: 'Class decorators recorded in application order, which is semantically significant. Decorators can transform or replace the class, so their presence does not by itself prove which live object will be admitted.',
  participation: 'The checker’s participation proposal. A candidate may qualify for protected class construction after runtime authentication. Dynamic means local participation was declined, for example by policy opt-out or an unsupported framework; inherited installed checks are not revoked.',
  checked_attr: 'The effective source policy for this exact class, after package defaults, module settings, and its own class override. True requests eligible class invariants and supported annotated field-write checks; false adds no new local contract and does not remove inherited checks.',
  dictionary: 'The checker’s model of class-instance dictionary behavior, such as dictionary-bearing or source-requested slots. It describes source semantics, not native offsets, a guaranteed optimized layout, or permission to bypass Python attribute rules.',
  instance_fields: 'Logical instance-field declarations, in source or transform order. The catalog can also include pseudo-fields such as ClassVar and InitVar; their presence does not mean they occupy checked instance storage.',
  methods: 'Method ownership, binding, implementation identity, and static signature facts. Parameter and return types remain static checker information: SOAC does not install runtime call-value checks from these signatures.',
  class_members: 'Members belonging to the class namespace, including descriptors and shadowable class defaults. These are distinct from instance storage; runtime admission checks the actual namespace against the proposal.',
  inheritance: 'The logical method-resolution-order information inferred for the class. Linearized bases exclude the class itself; this is not a physical field prefix or a native layout.',
  openness: 'What the checker assumes about possible subclasses and overrides. An open family means future subclasses must not be treated as a fixed, closed set of runtime types.',
  transform: 'Recognized source transformation metadata, such as dataclass options, generated members, and replacement-class behavior. This describes the supported transform model; runtime binding must still verify the actual result.',
  uncertainty: 'Reasons this record is not fully precise, such as unknown types or open-world behavior. These reasons constrain what may be assumed from the metadata; they are not automatically a runtime error.',
  name: 'The source-level field or member name. For inherited fields, the declaring_class record identifies the original owner rather than transferring ownership to the subclass.',
  value_type: 'The static type recorded for this value or field. An inferred type alone is not a runtime guarantee. Field-write enforcement additionally requires selected policy, supported explicit annotation provenance, suitable storage, and actual runtime binding.',
  annotation_origin: 'How the type was obtained: for example explicit source annotation, inferred information, or no annotation. Only supported explicit field annotations can select mandatory field-value checks; inferred constructor signatures do not make a field checked.',
  annotation_definition: 'The exact annotated assignment that introduced the field contract. Null means semantic provenance is unavailable or ambiguous; generated constructor parameters are not substituted for a field declaration.',
  declaring_class: 'The original source class owning this declaration, with its source digest. Inherited checked fields retain that owner; a subclass opt-out cannot discard the inherited contract.',
  field_kind: 'The semantic role of this declaration: real instance field, callable field, shadowable default, cached descriptor field, ClassVar, InitVar, framework-private field, or dynamic field. ClassVar and InitVar are pseudo-fields, not ordinary instance storage.',
  read_policy: 'How reads follow Python semantics: ordinary attribute lookup, instance storage followed by a class default, descriptor precedence, or a cached descriptor. This is a semantic lookup policy, not a claim that a value is currently present.',
  write_policy: 'How writes to the declaration are modeled: ordinary attribute behavior, a declared field, descriptor dispatch, rejected ClassVar writes, InitVar, or dynamic handling. Actual checked storage writes still require the selected field policy and runtime binding.',
  initialization: 'Whether a field may be absent, is initialized by a constructor, is managed by a descriptor, or has unknown initialization. A declared type does not imply that an instance has already stored a value.',
  default: 'The declaration’s default value or default-factory metadata, or an explicit missing default. Factory results are not checked at the call boundary; supported checks occur when a value is written into selected storage.',
  descriptor: 'The descriptor semantics attached to this field or member, including property getter/setter/deleter references when available. Descriptors are not silently treated as plain instance storage.',
  signature: 'The static callable signature used by the checker, including parameter kinds, types, defaults, and return information. It is not a runtime parameter or return-value enforcement contract.',
  binding: 'How a method binds its receiver, such as instance, class, or static binding. This describes method semantics and ownership rather than a runtime value check on arguments.',
  implementation: 'The exact source identity of the method’s implementation. Aliasing a function into a class namespace does not transfer its lexical source ownership.',
  generated: 'Provenance for a method generated by a recognized transformation rather than an ordinary authored function definition.',
  source_range: 'A half-open range of UTF-8 byte offsets in the exact module source: start is included and end is excluded. It is not a character count or a range in the combined scenario file.',
  source_hash: 'The 64-bit module source identity hash used by SOAC. The raw record preserves the full integer exactly, even when it is larger than JavaScript can represent safely as a Number.',
  source_digest: 'The cryptographic digest of the source bytes associated with this reference. It binds a declaration to particular source content, not to a live Python object.',
  lexical_qualname: 'The declaration’s qualified name within its lexical source scopes. Repeated or nested names are still distinguished by their exact source ranges.',
  class_overrides: 'Source policy overrides attached to exact class declaration ranges. An override applies only to that declaration, not to nested classes or unrelated declarations with the same name.',
  strict_assign: 'The effective module policy for post-initialization global-binding restrictions. It is independent of checked_attr, which selects class and supported field-write invariants.',
  kind: 'The tagged variant of this metadata record. Its interpretation depends on the surrounding property: for example a static type shape, a descriptor kind, or a source transformation. The accompanying data fields describe that specific variant.',
  linearized_bases: 'The inferred logical MRO, excluding this class itself. Lookup order matters, but this list does not assign physical storage offsets or guarantee the actual runtime bases.',
  complete: 'Whether the checker considers the logical inheritance information complete. False means unknown or dynamic bases may prevent a full static account of the hierarchy.',
  override_policy: 'The checker’s policy for overriding this method, such as requiring a compatible static signature. This is not a runtime check on parameter or return values.',
  declared_final: 'Whether the declaration is marked final in the supported source model. It records static intent; the runtime still has to authenticate any installed class or method invariant.',
  dataclass_options: 'The statically resolved dataclass switches, including generated initializer/equality methods, frozen behavior, keyword-only fields, slots, and weak-reference slots. Unknown decorator behavior is not guessed into this record.',
  generated_methods: 'Names of methods expected from the recognized class transform. Their actual generated implementations and ownership must still be bound during supported construction.',
  parameters: 'Static parameter names, argument-binding kinds, annotations, and defaults. Python argument binding remains in effect; these types do not add runtime call-value enforcement.',
  return_type: 'The checker’s static return type. The current SOAC contract scope does not install return-value checks from this metadata.',
  allow_subclasses: 'Whether this nominal builtin type accepts subclasses. In particular, a normal int annotation allows bool, whereas an exact-int type would not.',
  function_kind: 'Whether the authored function is synchronous, a coroutine, a generator, or an async generator. This is separate from its static parameter and return types.',
  mutability: 'The checker’s model of whether this global binding can change. Actual restrictions additionally require selected module policy and authenticated runtime binding.',
};
const VALUE_HELP = {
  open_world: 'The exporter currently adds this conservative marker to every class record, including classes declared final. It does not report a particular missing field or a failed admission; consumers must not infer a complete closed-world model from the record alone.',
  any: 'Some relevant type information is Any, so it deliberately provides no precise static restriction on the value.',
  unknown: 'The checker could not determine some relevant type or behavior precisely. Unknown information must not be treated as a proven runtime guarantee.',
  checker_todo: 'The checker represents some relevant information as an unimplemented analysis case. The missing analysis is not replaced with an assumed precise type.',
  ignored_diagnostic: 'A checker diagnostic was suppressed in a relevant region. Precise facts affected by suppression must not be treated as established contracts.',
  unresolved_import: 'A relevant import could not be resolved completely, leaving information about the imported definitions uncertain.',
  dynamic_decorator: 'A decorator can alter or replace a definition in a way the checker cannot fully model.',
  dynamic_metaclass: 'Metaclass behavior is not fully known to the checker and may affect class creation or the resulting namespace.',
  dynamic_descriptor: 'A descriptor can control attribute access or mutation in a way the checker cannot fully model.',
  unsafe_narrowing: 'Relevant narrowing information is not safe to retain as an unconditional fact across the supported runtime behavior.',
  unsupported_type: 'Some relevant type shape is outside the supported contract representation or analysis. Its unsupported details must not be inferred as a precise guarantee.',
  partial_initialization: 'Initialization may leave some relevant state absent or only partially established. A declaration alone does not prove that a stored value is present.',
  dictionary_bearing: 'Instances can have dictionary-backed attributes. An escaped instance dictionary may retain installed field-write constraints independently of the instance.',
  explicit_slots: 'The source requests __slots__. The checker records that choice; it does not invent slot offsets or assume that inherited __dict__ storage disappears.',
  open_subclass_family: 'The class can have subclasses not enumerated here. Code must not assume a closed list of subclasses or a single fixed override target.',
  builtin_type: 'The metaclass is the ordinary builtin type, according to static analysis. Actual bases and the constructed namespace still need runtime verification.',
  shadowable_class_default: 'A class-level default is visible until an instance stores its own value. This is distinct from a ClassVar, which is not an instance field.',
  instance_field: 'A declaration for ordinary per-instance storage. Whether writes are checked depends on its annotation, the selected policy, and runtime binding.',
  callable_instance_field: 'A value stored on the instance whose static type is callable. It is an instance field, not a class method merely because it can be called.',
  class_variable: 'A ClassVar declaration describes the class namespace rather than ordinary instance storage.',
  init_only: 'An InitVar-like initializer input is not stored as an instance field merely because it is annotated.',
  declared_field: 'The write targets a declared field. Supported explicit field annotations can become storage-write predicates after the class is selected and bound.',
  explicit: 'The relevant type comes from an authored annotation, rather than being inferred from an assigned value or a constructor signature.',
  inferred: 'This type was inferred by the checker. Inference alone does not install a mandatory runtime field-value constraint.',
};
const node = (tag, text, cls) => {
  const item = document.createElement(tag);
  if (text !== undefined) item.textContent = text;
  if (cls) item.className = cls;
  return item;
};
async function api(path, options) {
  const response = await fetch(path, options);
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
  return data;
}
function showError(error) { $('error').textContent = error.message; $('error').hidden = false; }
function explain(element, property, value) {
  if (PROPERTY_HELP[property]) {
    element.title = [VALUE_HELP[value], PROPERTY_HELP[property]].filter(Boolean).join('\n\n'); element.classList.add('property');
    element.setAttribute('aria-description', element.title);
  }
  return element;
}
function raw(parent, title, value, exactJSON) {
  const details = node('details');
  const pre = node('pre'), text = exactJSON ?? JSON.stringify(value, null, 2);
  let offset = 0;
  for (const match of text.matchAll(/"(?:\\.|[^"\\])*"(?=\s*:)/g)) {
    pre.append(node('span', text.slice(offset, match.index)), explain(node('span', match[0]), JSON.parse(match[0])));
    offset = match.index + match[0].length;
  }
  pre.append(node('span', text.slice(offset)));
  details.append(node('summary', title), pre);
  parent.append(details);
}
function typeName(type) {
  if (!type || typeof type !== 'object') return String(type ?? 'unknown');
  const { kind, data } = type;
  if (['exact_builtin', 'nominal_builtin'].includes(kind)) return (data.builtin || data) + (kind === 'exact_builtin' ? ' (exact)' : '');
  if (kind === 'union') return data.map(typeName).join(' | ');
  if (kind === 'optional') return `${typeName(data)} | None`;
  if (kind === 'callable') return signatureName(data);
  if (kind === 'none') return 'None';
  if (['nominal_class', 'exact_class'].includes(kind)) return data.definition.lexical_qualname;
  if (kind === 'literal') return `Literal[${data.kind === 'bool' ? (data.value ? 'True' : 'False') : ['int', 'float'].includes(data.kind) ? data.value : JSON.stringify(data.value ?? data.kind)}]`;
  if (['any', 'unknown', 'todo'].includes(kind)) return kind;
  return JSON.stringify(type);
}
function badge(parent, text, style = '', property, value) { parent.append(explain(node('span', text, `badge ${style}`), property, value)); }
function signatureName(signature) {
  const parameters = [];
  let keywordOnly = false;
  signature.parameters.forEach((parameter, index) => {
    if (parameter.kind === 'keyword_only' && !keywordOnly) { parameters.push('*'); keywordOnly = true; }
    if (parameter.kind === 'var_args') keywordOnly = true;
    const prefix = parameter.kind === 'var_args' ? '*' : parameter.kind === 'var_keywords' ? '**' : '';
    const optional = parameter.default && parameter.default.kind !== 'missing' ? ' = …' : '';
    parameters.push(`${prefix}${parameter.name}: ${typeName(parameter.value_type)}${optional}`);
    if (parameter.kind === 'positional_only' && signature.parameters[index + 1]?.kind !== 'positional_only') parameters.push('/');
  });
  return `(${parameters.join(', ')}) → ${typeName(signature.return_type)}`;
}
function metadata(definition) {
  const box = node('div', undefined, 'definition-metadata');
  const fact = definition.fact;
  if (!current.publication) { box.append(node('div', current.analysis_error ? 'Analysis failed' : 'Inferring…', 'inferred-type muted')); return box; }
  if (!fact) {
    box.append(node('div', definition.status === 'ordinary' ? 'No published type' : 'No inferred record', 'inferred-type muted'));
    return box;
  }
  const inferred = fact.signature ? signatureName(fact.signature) : definition.kind === 'class' ? 'type' : typeName(fact.value_type);
  box.append(explain(node('div', inferred, 'inferred-type'), fact.signature ? 'signature' : 'value_type'));
  const properties = node('div', undefined, 'type-properties');
  const keys = {
    class: ['participation', 'checked_attr', 'dictionary', 'openness', 'metaclass'],
    field: ['annotation_origin', 'field_kind', 'read_policy', 'write_policy', 'initialization'],
    method: ['binding', 'override_policy', 'declared_final'],
    function: ['function_kind'],
    member: ['kind', 'descriptor'],
    binding: ['mutability'],
  }[definition.kind] || [];
  for (const key of keys) {
    const rawValue = key === 'checked_attr' ? definition.checked_attr : fact[key];
    if (rawValue === null || rawValue === undefined) continue;
    const value = typeof rawValue === 'object' ? rawValue.kind : rawValue;
    const label = `${key}: ${value}${rawValue.reasons?.length ? ` (${rawValue.reasons.join(', ')})` : ''}`;
    badge(properties, label, '', key, value);
  }
  for (const reason of fact.uncertainty || []) badge(properties, `uncertainty: ${reason}`, 'warn', 'uncertainty', reason);
  box.append(properties);
  raw(box, 'Raw JSON', fact, definition.fact_json);
  return box;
}
function source(lines, first = 1, definitionLines = []) {
  const box = node('div', undefined, 'source');
  lines.forEach((line, index) => {
    const row = node('div', undefined, `line${definitionLines.includes(first + index) ? ' definition' : ''}`);
    row.append(node('span', first + index, 'lineno'), node('span', line, `line-text${line.trimStart().startsWith('#') ? ' comment' : ''}`));
    box.append(row);
  });
  return box;
}
function linesOf(text) { const lines = text.split(/\r\n|\n|\r/); if (lines.at(-1) === '') lines.pop(); return lines; }
function renderDocument() {
  if (!current) return;
  const content = $('content'); content.replaceChildren();
  for (const module of current.modules) {
    const panel = node('section', undefined, 'module'), head = node('div', undefined, 'module-head');
    head.append(node('strong', `Setup · ${module.name}`), node('span', module.path, 'muted')); panel.append(head);
    const columns = node('div', undefined, 'column-head'); columns.append(node('div', 'Module source'), node('div', 'Inferred type metadata')); panel.append(columns);
    const lines = linesOf(module.source), anchors = new Map();
    const records = [...(module.records || [])];
    // Keep un-published classes visible without inventing field/function facts.
    for (const definition of module.classes) if (!records.some(record => record.kind === 'class' && record.start === definition.start)) records.push(definition);
    for (const record of records) {
      const start = record.block_line ?? record.line;
      if (!anchors.has(start)) anchors.set(start, []);
      anchors.get(start).push(record);
    }
    const starts = [...new Set([1, ...anchors.keys()])].sort((a, b) => a - b);
    starts.forEach((line, index) => {
      const row = node('div', undefined, `source-row tone-${index % 2}`), definitions = anchors.get(line);
      row.append(source(lines.slice(line - 1, (starts[index + 1] || lines.length + 1) - 1), line, (definitions || []).map(definition => definition.line)));
      const information = node('div', undefined, 'metadata');
      for (const definition of definitions || []) {
        const card = metadata(definition);
        card.style.paddingTop = `${(definition.line - line) * 21}px`;
        information.append(card);
      }
      row.append(information);
      panel.append(row);
    });
    if (module.facts) {
      const info = node('div', undefined, 'metadata');
      raw(info, 'Resolved module & class policy', module.facts.language_policy);
      if (module.facts.diagnostics.length) raw(info, 'Module diagnostics', module.facts.diagnostics);
      if (module.unmatched_facts.length) raw(info, 'Records without a source location', module.unmatched_facts, module.unmatched_json);
      panel.append(info);
    }
    content.append(panel);
  }
  current.blocks.forEach((block, index) => {
    const panel = node('section', undefined, 'validation'), head = node('div', undefined, 'module-head');
    head.append(node('strong', `Test case ${index + 1} · # ${block.label}`), node('span', `Validation · scenario line ${block.line}`, 'muted'));
    panel.append(head, source(linesOf(block.source), block.line + 1)); content.append(panel);
  });
  $('evidence').hidden = !current.publication;
  $('snapshot').textContent = current.publication ? `Analysis snapshot · ${current.publication.generation.slice(0, 12)}` : 'Not analyzed';
  if (current.publication) $('diagnostics').textContent = `Source SHA-256: ${current.digest}\nGeneration: ${current.publication.generation}\nRetained evidence: ${current.evidence}\n\n${current.diagnostics}`;
}
function visibleCatalog() {
  const query = $('search').value.toLowerCase(), onlyTypes = $('types-only').checked;
  return catalog.filter(item => (!onlyTypes || item.classes.length) && `${item.id} ${item.classes.join(' ')}`.toLowerCase().includes(query));
}
function navigation() {
  const matches = visibleCatalog(), index = matches.findIndex(item => item.id === selectedId);
  $('position').textContent = index < 0 ? '' : `${index + 1} / ${matches.length}`;
  $('previous').disabled = index <= 0;
  $('next').disabled = index < 0 || index >= matches.length - 1;
}
function move(direction) {
  const matches = visibleCatalog(), index = matches.findIndex(item => item.id === selectedId);
  if (index >= 0 && matches[index + direction]) select(matches[index + direction].id);
}
function renderCatalog() {
  const matches = visibleCatalog();
  $('count').textContent = `${matches.length} scenarios · ${matches.reduce((sum, item) => sum + item.cases, 0)} cases`;
  $('catalog').replaceChildren(); let previousGroup;
  for (const item of matches) {
    const parts = item.id.split('/'), group = parts.slice(0, -1).join(' / ');
    if (group !== previousGroup) { $('catalog').append(node('div', group, 'group')); previousGroup = group; }
    const button = node('button', parts.at(-1).replace(/\.py$/, '').replaceAll('_', ' '));
    button.title = item.id; button.setAttribute('aria-current', String(item.id === selectedId));
    button.append(node('small', item.error ? 'Invalid scenario — view details' : `${item.cases} cases · ${item.classes.length} types`));
    button.addEventListener('click', () => select(item.id)); $('catalog').append(button);
  }
  if (!matches.length) $('catalog').append(node('p', 'No matching scenarios.', 'muted'));
  navigation();
}
async function select(id) {
  const token = ++request; selectedId = id; pending = null; navigation(); $('error').hidden = true; $('status').textContent = 'Loading source…';
  try {
    const doc = await api(`/api/case?id=${encodeURIComponent(id)}`); if (token !== request) return;
    const snapshot = snapshots.get(id); current = snapshot?.digest === doc.digest ? snapshot : doc;
    $('theme').textContent = id.split('/').slice(0, -1).join(' / ').toUpperCase();
    $('title').textContent = id.split('/').at(-1).replace(/\.py$/, '').replaceAll('_', ' ');
    $('summary').textContent = `${doc.modules.length} modules · ${doc.blocks.length} validation cases · ${doc.modes.join(', ')}`;
    history.replaceState(null, '', `?id=${encodeURIComponent(id)}`);
    $('status').textContent = current.publication ? '' : 'Inferring metadata…';
    renderCatalog(); renderDocument();
    $('main').scrollTop = 0;
    if (!current.publication) { pending = {id, digest: current.digest}; inferPending(); }
  } catch (error) { if (token === request) { current = null; $('content').replaceChildren(); showError(error); $('status').textContent = ''; } }
}
async function inferPending() {
  if (!pending || analyzing) return;
  const {id, digest} = pending; pending = null;
  analyzing = true; $('error').hidden = true;
  $('status').textContent = 'Inferring metadata…';
  try {
    const doc = await api(`/api/analyze?id=${encodeURIComponent(id)}`, {method: 'POST'});
    snapshots.set(id, doc);
    if (selectedId === id && current?.id === id && current.digest === digest) {
      current = doc; renderDocument();
      $('status').textContent = '';
    }
  } catch (error) {
    if (selectedId === id) {
      showError(error); $('status').textContent = 'Analysis failed. No inferred metadata has been substituted.';
      if (current?.id === id) { current.analysis_error = true; renderDocument(); }
    }
  }
  finally {
    analyzing = false;
    if (pending && snapshots.get(pending.id)?.digest === pending.digest) pending = null;
    inferPending();
  }
}
$('search').addEventListener('input', renderCatalog);
$('types-only').addEventListener('change', renderCatalog);
$('previous').addEventListener('click', () => move(-1));
$('next').addEventListener('click', () => move(1));
document.addEventListener('keydown', event => {
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || event.target.closest('input,textarea,select,[contenteditable="true"]')) return;
  if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); move(event.key === 'ArrowRight' ? 1 : -1); }
});
(async () => {
  try {
    catalog = await api('/api/cases'); renderCatalog();
    const query = new URLSearchParams(location.search);
    const initial = query.get('id') || (catalog.find(item => item.id === 'policy/class_only_opt_in.py') || catalog[0])?.id;
    if (initial) await select(initial);
  } catch (error) { showError(error); $('count').textContent = 'Catalog unavailable'; }
})();
