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

    if (step === 5) {
        const urlInput = document.getElementById('model-base-url');
        const chatSel = document.getElementById('model-chat');
        if (urlInput && urlInput.value.trim() && chatSel && chatSel.options.length === 0) {
            triggerModelDiscovery();
        }
    }
}

let modelDiscoveryTimer = null;
let modelDiscoverySeq = 0;

function triggerModelDiscovery(desiredChat, desiredEmbed, delay = 350) {
    if (modelDiscoveryTimer) clearTimeout(modelDiscoveryTimer);
    if (delay === 0) {
        discoverModels(desiredChat, desiredEmbed);
    } else {
        modelDiscoveryTimer = setTimeout(() => {
            discoverModels(desiredChat, desiredEmbed);
        }, delay);
    }
}

async function discoverModels(desiredChat, desiredEmbed) {
    const urlInput = document.getElementById('model-base-url');
    const keyInput = document.getElementById('model-api-key');
    const chatSel = document.getElementById('model-chat');
    const embedSel = document.getElementById('model-embed');
    const statusBox = document.getElementById('model-discovery-status');

    if (!urlInput || !chatSel || !embedSel) return;

    const baseUrl = urlInput.value.trim();
    const apiKey = keyInput ? keyInput.value.trim() : '';

    if (!baseUrl) {
        chatSel.innerHTML = '';
        embedSel.innerHTML = '';
        if (statusBox) {
            statusBox.style.display = 'none';
        }
        return;
    }

    const prevChat = desiredChat || (chatSel.value && chatSel.value !== "" ? chatSel.value : (chatSel.dataset.selectedValue || ''));
    const prevEmbed = desiredEmbed || (embedSel.value && embedSel.value !== "" ? embedSel.value : (embedSel.dataset.selectedValue || ''));
    if (prevChat) chatSel.dataset.selectedValue = prevChat;
    if (prevEmbed) embedSel.dataset.selectedValue = prevEmbed;

    const seq = ++modelDiscoverySeq;

    // Show loading state
    if (statusBox) {
        statusBox.style.display = 'block';
        statusBox.className = 'probe-result';
        statusBox.textContent = 'Discovering available models from endpoint...';
    }
    chatSel.disabled = true;
    embedSel.disabled = true;
    chatSel.innerHTML = '<option value="">Discovering models...</option>';
    embedSel.innerHTML = '<option value="">Discovering models...</option>';

    try {
        const resp = await fetch('/api/models', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                base_url: baseUrl || null,
                api_key: apiKey || null
            })
        });

        const data = await resp.json();
        if (seq !== modelDiscoverySeq) return;

        if (!data.ok) {
            chatSel.innerHTML = '';
            embedSel.innerHTML = '';
            chatSel.disabled = false;
            embedSel.disabled = false;
            if (statusBox) {
                statusBox.style.display = 'block';
                statusBox.className = 'probe-result probe-error';
                statusBox.textContent = `✗ Model discovery failed: ${data.error || 'Failed to query endpoint'}`;
            }
            return;
        }

        // Success: populate dropdowns
        chatSel.disabled = false;
        embedSel.disabled = false;
        chatSel.innerHTML = '';

        const chatList = (data.chat_models && data.chat_models.length > 0) ? data.chat_models : data.models;
        const embedList = (data.embedding_models && data.embedding_models.length > 0) ? data.embedding_models : data.models.filter(m => m.is_embedding);

        chatList.forEach(m => {
            const opt = document.createElement('option');
            opt.value = m.id;
            opt.textContent = m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id;
            chatSel.appendChild(opt);
        });

        if (embedList.length > 0) {
            embedSel.innerHTML = '<option value="">-- None / Disabled --</option>';
            embedList.forEach(m => {
                const opt = document.createElement('option');
                opt.value = m.id;
                opt.textContent = m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id;
                embedSel.appendChild(opt);
            });
        } else {
            embedSel.innerHTML = '<option value="">-- No embedding models discovered --</option>';
        }

        const targetChat = desiredChat || chatSel.dataset.selectedValue || '';
        if (targetChat) {
            for (let i = 0; i < chatSel.options.length; i++) {
                if (chatSel.options[i].value === targetChat) {
                    chatSel.selectedIndex = i;
                    break;
                }
            }
        }

        const targetEmbed = desiredEmbed || embedSel.dataset.selectedValue || '';
        if (targetEmbed) {
            for (let i = 0; i < embedSel.options.length; i++) {
                if (embedSel.options[i].value === targetEmbed) {
                    embedSel.selectedIndex = i;
                    break;
                }
            }
        }

        if (statusBox) {
            statusBox.style.display = 'block';
            statusBox.className = 'probe-result probe-success';
            statusBox.textContent = `✓ Discovered ${data.models.length} models (${chatList.length} chat, ${embedList.length} embedding)`;
        }
    } catch (e) {
        if (seq !== modelDiscoverySeq) return;
        chatSel.innerHTML = '';
        embedSel.innerHTML = '';
        chatSel.disabled = false;
        embedSel.disabled = false;
        if (statusBox) {
            statusBox.style.display = 'block';
            statusBox.className = 'probe-result probe-error';
            statusBox.textContent = `✗ Model discovery request failed: ${e.message}`;
        }
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

document.addEventListener('DOMContentLoaded', () => {
    const urlInput = document.getElementById('model-base-url');
    const keyInput = document.getElementById('model-api-key');

    if (urlInput) {
        urlInput.addEventListener('input', () => triggerModelDiscovery(undefined, undefined, 350));
        urlInput.addEventListener('change', () => triggerModelDiscovery(undefined, undefined, 0));
        urlInput.addEventListener('blur', () => triggerModelDiscovery(undefined, undefined, 0));
        urlInput.addEventListener('paste', () => setTimeout(() => triggerModelDiscovery(undefined, undefined, 0), 10));
    }
    if (keyInput) {
        keyInput.addEventListener('input', () => triggerModelDiscovery(undefined, undefined, 350));
        keyInput.addEventListener('change', () => triggerModelDiscovery(undefined, undefined, 0));
        keyInput.addEventListener('blur', () => triggerModelDiscovery(undefined, undefined, 0));
        keyInput.addEventListener('paste', () => setTimeout(() => triggerModelDiscovery(undefined, undefined, 0), 10));
    }
});


