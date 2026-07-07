/**
 * OgreNotes SLA dashboard (Phase 5 M-P9) — now CDK-authoritative.
 *
 * Previously infra/cloudwatch-dashboard.json, applied via
 * `aws cloudwatch put-dashboard`. Inlined here so the CDK app is the single
 * source of truth. Widgets use metric SEARCH expressions scoped to the
 * `OgreNotes` namespace (crates/common/src/metrics/emf.rs); metric names +
 * dimensions mirror crates/api/src/middleware/metrics.rs and
 * crates/api/src/routes/metrics.rs. SLA annotations come from
 * design/performance-budgets.md.
 *
 * The per-widget `region` is templated — `slaDashboardBody(region)` injects
 * the stack region so the dashboard moves with the stack.
 */

// Widget definitions in CloudWatch dashboard-body shape. `region` is a
// placeholder overwritten by slaDashboardBody().
const WIDGETS: any[] = [
  {
    type: 'text',
    x: 0, y: 0, width: 24, height: 1,
    properties: {
      markdown:
        '## API p95 latency — top row. Horizontal annotations mark the per-route SLA from design/performance-budgets.md.',
    },
  },
  {
    type: 'metric',
    x: 0, y: 1, width: 6, height: 6,
    properties: {
      title: 'Auth p95 (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/auth/login"\', \'p95\', 60)', label: 'login p95', id: 'e1' }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/auth/refresh"\', \'p95\', 60)', label: 'refresh p95', id: 'e2' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: { horizontal: [{ label: 'SLA 200ms', value: 200, color: '#d62728' }] },
    },
  },
  {
    type: 'metric',
    x: 6, y: 1, width: 6, height: 6,
    properties: {
      title: 'GET /documents p95 (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/documents" method="GET"\', \'p95\', 60)', label: 'list p95', id: 'e1' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: { horizontal: [{ label: 'SLA 300ms', value: 300, color: '#d62728' }] },
    },
  },
  {
    type: 'metric',
    x: 12, y: 1, width: 6, height: 6,
    properties: {
      title: 'GET /documents/{id}/content p95 (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/documents/:id/content" method="GET"\', \'p95\', 60)', label: 'doc content p95', id: 'e1' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: { horizontal: [{ label: 'SLA 500ms', value: 500, color: '#d62728' }] },
    },
  },
  {
    type: 'metric',
    x: 18, y: 1, width: 6, height: 6,
    properties: {
      title: 'Search + WS upgrade p95 (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/search"\', \'p95\', 60)', label: 'search p95', id: 'e1' }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.request_latency_ms" route="/api/v1/documents/:id/ws"\', \'p95\', 60)', label: 'ws upgrade p95', id: 'e2' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: {
        horizontal: [
          { label: 'SLA search 800ms', value: 800, color: '#d62728' },
          { label: 'SLA ws 100ms', value: 100, color: '#ff7f0e' },
        ],
      },
    },
  },
  {
    type: 'text',
    x: 0, y: 7, width: 24, height: 1,
    properties: {
      markdown:
        '## Frontend RUM — user-perceived web vitals from `rum.lcp_ms` / `rum.inp_ms` / `rum.cls`. Sampled at 10% of sessions; p75 across the sampled population.',
    },
  },
  {
    type: 'metric',
    x: 0, y: 8, width: 8, height: 6,
    properties: {
      title: 'LCP p75 by page (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.lcp_ms" page="home"\', \'p75\', 300)', label: 'home', id: 'e1' }],
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.lcp_ms" page="editor"\', \'p75\', 300)', label: 'editor', id: 'e2' }],
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.lcp_ms" page="spreadsheet"\', \'p75\', 300)', label: 'spreadsheet', id: 'e3' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: {
        horizontal: [
          { label: 'editor SLA cold 3.5s', value: 3500, color: '#d62728' },
          { label: 'spreadsheet SLA cold 4.5s', value: 4500, color: '#ff7f0e' },
        ],
      },
    },
  },
  {
    type: 'metric',
    x: 8, y: 8, width: 8, height: 6,
    properties: {
      title: 'INP p75 by page (ms)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.inp_ms" page="home"\', \'p75\', 300)', label: 'home', id: 'e1' }],
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.inp_ms" page="editor"\', \'p75\', 300)', label: 'editor', id: 'e2' }],
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.inp_ms" page="spreadsheet"\', \'p75\', 300)', label: 'spreadsheet', id: 'e3' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: { horizontal: [{ label: 'SLA 200ms', value: 200, color: '#d62728' }] },
    },
  },
  {
    type: 'metric',
    x: 16, y: 8, width: 8, height: 6,
    properties: {
      title: 'CLS p75 (unitless)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,page,ua_class} MetricName="rum.cls"\', \'p75\', 300)', label: 'cls', id: 'e1' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'score' } },
      annotations: { horizontal: [{ label: 'SLA 0.1', value: 0.1, color: '#d62728' }] },
    },
  },
  {
    type: 'text',
    x: 0, y: 14, width: 24, height: 1,
    properties: {
      markdown:
        '## Throughput + error rate. 5xx counter widget is the page-or-don\'t signal — anything sustained > 1% is the api-5xx-rate alarm.',
    },
  },
  {
    type: 'metric',
    x: 0, y: 15, width: 12, height: 6,
    properties: {
      title: 'Request rate by status (req/min)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total"\', \'Sum\', 60)', label: 'all', id: 'e1' }],
      ],
      view: 'timeSeries',
      stacked: true,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, label: 'req/min' } },
    },
  },
  {
    type: 'metric',
    x: 12, y: 15, width: 12, height: 6,
    properties: {
      title: '5xx error rate (%)',
      metrics: [
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total" status="500"\', \'Sum\', 60)', label: '500', id: 'e500', visible: false }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total" status="502"\', \'Sum\', 60)', label: '502', id: 'e502', visible: false }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total" status="503"\', \'Sum\', 60)', label: '503', id: 'e503', visible: false }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total" status="504"\', \'Sum\', 60)', label: '504', id: 'e504', visible: false }],
        [{ expression: 'SEARCH(\'{OgreNotes,method,route,status} MetricName="api.requests_total"\', \'Sum\', 60)', label: 'all', id: 'all', visible: false }],
        [{ expression: '100 * (SUM(e500) + SUM(e502) + SUM(e503) + SUM(e504)) / SUM(all)', label: '5xx %', id: 'ratio' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      yAxis: { left: { min: 0, max: 5, label: '%' } },
      annotations: { horizontal: [{ label: 'alarm 1%', value: 1.0, color: '#d62728' }] },
    },
  },
  {
    type: 'text',
    x: 0, y: 21, width: 24, height: 1,
    properties: {
      markdown:
        '## Backend dependencies. Diagnostic-only — failures here usually surface upstream as the API p95 widgets fire.',
    },
  },
  {
    type: 'metric',
    x: 0, y: 22, width: 8, height: 6,
    properties: {
      title: 'DynamoDB write failures',
      metrics: [['OgreNotes', 'dynamo.write_failures_total']],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      stat: 'Sum',
      period: 60,
      yAxis: { left: { min: 0, label: 'count' } },
    },
  },
  {
    type: 'metric',
    x: 8, y: 22, width: 8, height: 6,
    properties: {
      title: 'Redis publish failures',
      metrics: [['OgreNotes', 'redis.publish_failures_total']],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      stat: 'Sum',
      period: 60,
      yAxis: { left: { min: 0, label: 'count' } },
    },
  },
  {
    type: 'metric',
    x: 16, y: 22, width: 8, height: 6,
    properties: {
      title: 'WS active connections',
      metrics: [['OgreNotes', 'ws.active_connections']],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      stat: 'Maximum',
      period: 60,
      yAxis: { left: { min: 0, label: 'conn' } },
    },
  },
  {
    type: 'text',
    x: 0, y: 28, width: 24, height: 1,
    properties: {
      markdown:
        '## Phase 6 RAG — agent endpoint, embedding pipeline, vector store. Widgets sourced from the OgreNotes namespace (`ask.*`) plus AWS-published `AWS/Bedrock` and `AWS/EFS` namespaces. The Claude spend widget is a placeholder — emitting per-call input + output token counts is a follow-up code change tracked in the M-6.3 validation milestone.',
    },
  },
  {
    type: 'metric',
    x: 0, y: 29, width: 8, height: 6,
    properties: {
      title: '/ask requests + errors (per min)',
      metrics: [
        ['OgreNotes', 'ask.requests_total', { label: 'requests', stat: 'Sum' }],
        ['.', 'ask.claude_api_errors_total', { label: 'errors', stat: 'Sum' }],
        ['.', 'ask.disabled_by_admin_total', { label: 'admin-gated', stat: 'Sum' }],
        ['.', 'ask.quota_global_exceeded_total', { label: 'global quota', stat: 'Sum' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'count' } },
    },
  },
  {
    type: 'metric',
    x: 8, y: 29, width: 8, height: 6,
    properties: {
      title: '/ask total latency (ms)',
      metrics: [
        ['OgreNotes', 'ask.total_latency_ms', { stat: 'p50', label: 'p50' }],
        ['...', { stat: 'p95', label: 'p95' }],
        ['...', { stat: 'p99', label: 'p99' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'ms' } },
      annotations: { horizontal: [{ label: 'RAG plan §4.2 target 10s', value: 10000, color: '#d62728' }] },
    },
  },
  {
    type: 'metric',
    x: 16, y: 29, width: 8, height: 6,
    properties: {
      title: 'Agent tool-use rounds',
      metrics: [
        ['OgreNotes', 'ask.agent_rounds', { stat: 'Average', label: 'avg' }],
        ['...', { stat: 'Maximum', label: 'max' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'rounds' } },
      annotations: { horizontal: [{ label: 'MAX_TOOL_ROUNDS=5', value: 5, color: '#7f7f7f' }] },
    },
  },
  {
    type: 'metric',
    x: 0, y: 35, width: 8, height: 6,
    properties: {
      title: 'Bedrock embedding invocations',
      metrics: [
        ['AWS/Bedrock', 'Invocations', 'ModelId', 'amazon.titan-embed-text-v2:0', { label: 'Titan v2 invocations' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      stat: 'Sum',
      period: 60,
      yAxis: { left: { min: 0, label: 'count' } },
    },
  },
  {
    type: 'metric',
    x: 8, y: 35, width: 8, height: 6,
    properties: {
      title: 'Bedrock embed-token spend (est $)',
      _comment:
        'InputTokenCount × $0.02/MTok (Titan Embed v2 published rate as of 2026-05). Update the multiplier if EMBEDDING_MODEL_ID changes.',
      metrics: [
        [{ expression: 'input * 0.00000002', label: 'USD/min (Titan v2 input)', id: 'spend' }],
        ['AWS/Bedrock', 'InputTokenCount', 'ModelId', 'amazon.titan-embed-text-v2:0', { id: 'input', stat: 'Sum', visible: false }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'USD' } },
    },
  },
  {
    type: 'metric',
    x: 16, y: 35, width: 8, height: 6,
    properties: {
      title: 'Claude /ask spend (est $)',
      _comment:
        'input_tokens × $3/MTok + output_tokens × $15/MTok (claude-sonnet-4-6 published rates as of 2026-05; match `tests/rag-eval/src/main.rs::CLAUDE_*_PER_MTOK`). Slight undercount on errored requests — token counts only emit on the agent loop\'s success paths, matching the SSE Usage contract.',
      metrics: [
        [{ expression: 'input_tok * 0.000003 + output_tok * 0.000015', label: 'USD/min (Sonnet)', id: 'spend' }],
        ['OgreNotes', 'ask.claude_input_tokens', { id: 'input_tok', stat: 'Sum', visible: false }],
        ['.', 'ask.claude_output_tokens', { id: 'output_tok', stat: 'Sum', visible: false }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'USD' } },
    },
  },
  {
    type: 'metric',
    x: 0, y: 41, width: 12, height: 6,
    properties: {
      title: 'Qdrant EFS storage bytes',
      _comment:
        'AWS/EFS StorageBytes is dimensioned by FileSystemId + StorageClass; SEARCH picks up whichever filesystem the stack creates.',
      metrics: [
        [{ expression: 'SEARCH(\'{AWS/EFS,FileSystemId,StorageClass} MetricName="StorageBytes" StorageClass="Total"\', \'Average\', 300)', label: 'total bytes', id: 'e1' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 300,
      yAxis: { left: { min: 0, label: 'bytes' } },
    },
  },
  {
    type: 'metric',
    x: 12, y: 41, width: 12, height: 6,
    properties: {
      title: 'Qdrant ECS task health',
      _comment:
        'AWS/ECS RunningTaskCount + DesiredTaskCount for the Qdrant service, matched by the prefix-aware ServiceName dimension.',
      metrics: [
        [{ expression: 'SEARCH(\'{AWS/ECS,ClusterName,ServiceName} MetricName="RunningTaskCount" ServiceName=~"ogrenote-qdrant"\', \'Average\', 60)', label: 'running', id: 'e1' }],
        [{ expression: 'SEARCH(\'{AWS/ECS,ClusterName,ServiceName} MetricName="DesiredTaskCount" ServiceName=~"ogrenote-qdrant"\', \'Average\', 60)', label: 'desired', id: 'e2' }],
      ],
      view: 'timeSeries',
      stacked: false,
      region: 'PLACEHOLDER',
      period: 60,
      yAxis: { left: { min: 0, label: 'tasks' } },
    },
  },
];

/** Build the dashboard body JSON with the stack region injected per widget. */
export function slaDashboardBody(region: string): string {
  const widgets = JSON.parse(JSON.stringify(WIDGETS));
  for (const widget of widgets) {
    if (widget.properties && 'region' in widget.properties) {
      widget.properties.region = region;
    }
  }
  return JSON.stringify({ widgets });
}
