let currentStep = 1;

function goToStep(step) {
    document.querySelectorAll('.wizard-step').forEach(el => el.classList.remove('active'));
    document.querySelectorAll('.step-num').forEach(el => el.classList.remove('active'));

    currentStep = step;
    const target = document.getElementById(`step-${step}`);
    if (target) target.classList.add('active');

    for (let i = 1; i <= step; i++) {
        const badge = document.getElementById(`badge-${i}`);
        if (badge) badge.classList.add('active');
    }
}

async function testGitHub() {
    const probe = document.getElementById('gh-probe');
    probe.style.display = 'block';
    probe.className = 'probe-result';
    probe.textContent = 'Testing GitHub connection...';

    const token = document.getElementById('gh-token').value.trim();
    try {
        const resp = await fetch('/api/test/github', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ token: token || null })
        });
        const data = await resp.json();
        if (data.ok) {
            probe.className = 'probe-result probe-success';
            probe.textContent = `✓ ${data.detail || 'Connected'} (Latency: ${data.latency_ms}ms, Rate Limit: ${data.rate_limit_remaining !== null ? data.rate_limit_remaining : 'N/A'})`;
        } else {
            probe.className = 'probe-result probe-error';
            probe.textContent = `✗ Connection failed: ${data.detail || 'Unknown error'}`;
        }
    } catch (e) {
        probe.className = 'probe-result probe-error';
        probe.textContent = `✗ Request failed: ${e.message}`;
    }
}

async function testJira() {
    const probe = document.getElementById('jira-probe');
    probe.style.display = 'block';
    probe.className = 'probe-result';
    probe.textContent = 'Testing Jira connection...';

    const url = document.getElementById('jira-url').value.trim();
    const email = document.getElementById('jira-email').value.trim();
    const token = document.getElementById('jira-token').value.trim();

    try {
        const resp = await fetch('/api/test/jira', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url: url || null, email: email || null, token: token || null })
        });
        const data = await resp.json();
        if (data.ok) {
            probe.className = 'probe-result probe-success';
            probe.textContent = `✓ ${data.detail || 'Connected'} (Latency: ${data.latency_ms}ms)`;
        } else {
            probe.className = 'probe-result probe-error';
            probe.textContent = `✗ Connection failed: ${data.detail || 'Unknown error'}`;
        }
    } catch (e) {
        probe.className = 'probe-result probe-error';
        probe.textContent = `✗ Request failed: ${e.message}`;
    }
}

async function testLaunchpad() {
    const probe = document.getElementById('lp-probe');
    probe.style.display = 'block';
    probe.className = 'probe-result';
    probe.textContent = 'Testing Launchpad connectivity...';

    const project = document.getElementById('lp-project').value.trim();

    try {
        const resp = await fetch('/api/test/launchpad', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ default_project: project || null })
        });
        const data = await resp.json();
        if (data.ok) {
            probe.className = 'probe-result probe-success';
            probe.textContent = `✓ ${data.detail || 'Reachable'} (Latency: ${data.latency_ms}ms)`;
        } else {
            probe.className = 'probe-result probe-error';
            probe.textContent = `✗ Connectivity check failed: ${data.detail || 'Unknown error'}`;
        }
    } catch (e) {
        probe.className = 'probe-result probe-error';
        probe.textContent = `✗ Request failed: ${e.message}`;
    }
}

async function testModel() {
    const probe = document.getElementById('model-probe');
    probe.style.display = 'block';
    probe.className = 'probe-result';
    probe.textContent = 'Testing model endpoint...';

    const baseUrl = document.getElementById('model-base-url').value.trim();
    const apiKey = document.getElementById('model-api-key').value.trim();
    const chatModel = document.getElementById('model-chat').value.trim();
    const embedModel = document.getElementById('model-embed').value.trim();

    try {
        const resp = await fetch('/api/test/model', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                base_url: baseUrl || null,
                api_key: apiKey || null,
                chat_model: chatModel || null,
                embed_model: embedModel || null
            })
        });
        const data = await resp.json();
        if (data.ok) {
            probe.className = 'probe-result probe-success';
            let msg = `✓ Chat model available: ${data.chat_model || 'OK'}`;
            if (data.embedding_model) {
                msg += `\n✓ Embedding model available: ${data.embedding_model}`;
            }
            probe.textContent = msg;
        } else {
            probe.className = 'probe-result probe-error';
            probe.textContent = `✗ Model check failed: ${data.detail || 'Unknown error'}`;
        }
    } catch (e) {
        probe.className = 'probe-result probe-error';
        probe.textContent = `✗ Request failed: ${e.message}`;
    }
}

function populateReview() {
    const ghEnabled = document.getElementById('gh-enabled').checked;
    const ghToken = document.getElementById('gh-token').value.trim();
    document.getElementById('rev-gh').textContent = ghEnabled ? (ghToken ? 'Enabled (Token provided)' : 'Enabled (No token)') : 'Disabled';

    const jiraEnabled = document.getElementById('jira-enabled').checked;
    const jiraUrl = document.getElementById('jira-url').value.trim();
    document.getElementById('rev-jira').textContent = jiraEnabled ? (jiraUrl ? `Enabled (${jiraUrl})` : 'Enabled') : 'Disabled';

    const lpEnabled = document.getElementById('lp-enabled').checked;
    document.getElementById('rev-lp').textContent = lpEnabled ? 'Enabled' : 'Disabled';

    const modelEnabled = document.getElementById('model-enabled').checked;
    const modelChat = document.getElementById('model-chat').value.trim();
    document.getElementById('rev-model').textContent = modelEnabled ? (modelChat ? `Enabled (${modelChat})` : 'Enabled') : 'Disabled';

    const depth = document.getElementById('graph-depth').value || '2';
    const maxNodes = document.getElementById('max-nodes').value || '100';
    document.getElementById('rev-budget').textContent = `Depth: ${depth}, Max nodes: ${maxNodes}`;
}

async function finishSetup() {
    const errBox = document.getElementById('finish-err');
    errBox.style.display = 'none';
    const btn = document.getElementById('btn-finish');
    btn.disabled = true;
    btn.textContent = 'Saving configuration...';

    const reposInput = document.getElementById('gh-repos').value.trim();
    const repos = reposInput ? reposInput.split(',').map(s => s.trim()).filter(s => s.length > 0) : [];

    const payload = {
        github: {
            enabled: document.getElementById('gh-enabled').checked,
            token: document.getElementById('gh-token').value.trim() || null,
            default_org: document.getElementById('gh-org').value.trim() || null,
            default_repos: repos
        },
        jira: {
            enabled: document.getElementById('jira-enabled').checked,
            url: document.getElementById('jira-url').value.trim() || null,
            email: document.getElementById('jira-email').value.trim() || null,
            token: document.getElementById('jira-token').value.trim() || null,
            default_project: document.getElementById('jira-project').value.trim() || null
        },
        launchpad: {
            enabled: document.getElementById('lp-enabled').checked,
            default_project: document.getElementById('lp-project').value.trim() || null
        },
        model: {
            enabled: document.getElementById('model-enabled').checked,
            base_url: document.getElementById('model-base-url').value.trim() || null,
            api_key: document.getElementById('model-api-key').value.trim() || null,
            chat_model: document.getElementById('model-chat').value.trim() || null,
            embedding_model: document.getElementById('model-embed').value.trim() || null
        },
        search: {
            graph_depth: parseInt(document.getElementById('graph-depth').value, 10) || 2,
            max_graph_nodes: parseInt(document.getElementById('max-nodes').value, 10) || 100,
            max_deep_reads: parseInt(document.getElementById('max-reads').value, 10) || 30
        }
    };

    try {
        const resp = await fetch('/api/setup', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
        });
        const res = await resp.json();
        if (resp.ok && res.ok) {
            window.location.href = '/';
        } else {
            errBox.style.display = 'block';
            errBox.textContent = `Setup error: ${res.error || 'Failed to save configuration'}`;
            btn.disabled = false;
            btn.textContent = 'Start IssueMesh →';
        }
    } catch (e) {
        errBox.style.display = 'block';
        errBox.textContent = `Setup request failed: ${e.message}`;
        btn.disabled = false;
        btn.textContent = 'Start IssueMesh →';
    }
}
