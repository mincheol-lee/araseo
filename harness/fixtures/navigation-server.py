"""Deterministic LSP peer for Araseo's production transport/service tests."""
import json, os, sys, pathlib, urllib.parse

def send(message):
    data = json.dumps(message, ensure_ascii=False).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(data) + data)
    sys.stdout.buffer.flush()

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            raise EOFError()
        if line in (b'\r\n', b'\n'):
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

root = None
documents = {}
zero = {'line': 0, 'character': 0}
selection = {'start': zero, 'end': {'line': 0, 'character': 3}}
while True:
    try:
        message = read()
    except EOFError:
        break
    method = message.get('method')
    params = message.get('params', {})
    result = None
    if method is None:
        if message.get('id') == 'idle-config':
            assert message['result'][0]['numThreads'] == 2
            pathlib.Path(urllib.parse.unquote(urllib.parse.urlparse(root).path), 'idle-config-ready').write_text('ready')
        continue
    if method == 'exit':
        break
    if method == 'initialize':
        root = params['rootUri']
        send({'jsonrpc': '2.0', 'id': 'config', 'method': 'workspace/configuration',
              'params': {'items': [{'section': 'rust-analyzer'}]}})
        result = {'capabilities': {'definitionProvider': True, 'referencesProvider': True,
                  'workspaceSymbolProvider': True, 'callHierarchyProvider': True,
                  'textDocumentSync': 2}}
    elif method == 'textDocument/didOpen':
        doc = params['textDocument']
        documents[doc['uri']] = (doc['text'], doc['version'])
    elif method == 'textDocument/didClose':
        documents.pop(params['textDocument']['uri'])
    elif method == 'textDocument/didChange':
        uri = params['textDocument']['uri']
        old, version = documents[uri]
        change = params['contentChanges'][0]
        last_line = old.split('\n')[-1]
        assert change['range']['end'] == {'line': old.count('\n'), 'character': len(last_line.encode('utf-16-le')) // 2}
        assert params['textDocument']['version'] > version
        documents[uri] = (change['text'], params['textDocument']['version'])
    elif method == 'workspace/symbol':
        for i in range(80):
            send({'jsonrpc': '2.0', 'method': '$/progress', 'params': {'value': i}})
        name = 'edited' if any('edited' in text for text, _ in documents.values()) else 'run'
        result = [{'name': name, 'kind': 12, 'containerName': str(os.getpid()),
                   'location': {'uri': root + '/main.rs', 'range': selection}},
                  {'name': 'outside', 'kind': 12, 'location': {'uri': root + '-sibling/main.rs', 'range': selection}},
                  {'name': 'Class', 'kind': 5, 'location': {'uri': root + '/main.rs', 'range': selection}}]
    elif method == 'textDocument/definition':
        assert params['textDocument']['uri'] in documents
        result = [{'targetUri': root + '/other.rs', 'targetRange': selection, 'targetSelectionRange': selection}]
    elif method == 'textDocument/references':
        assert params['context']['includeDeclaration'] is True
        result = [{'uri': root + '/main.rs', 'range': selection}, {'uri': root + '-sibling/main.rs', 'range': selection}]
    elif method == 'textDocument/prepareCallHierarchy':
        result = [{'name': 'run', 'kind': 12, 'uri': root + '/main.rs',
                   'range': selection, 'selectionRange': selection, 'data': {'opaque': 'preserve-me'}}]
    elif method == 'callHierarchy/incomingCalls':
        assert params['item']['data']['opaque'] == 'preserve-me'
        result = [{'from': {'name': 'caller', 'kind': 12, 'uri': root + '/main.rs',
                   'range': selection, 'selectionRange': selection, 'data': {'opaque': 'preserve-me'}},
                   'fromRanges': [selection]}]
    if 'id' in message:
        send({'jsonrpc': '2.0', 'id': message['id'], 'result': result})
        if method == 'workspace/symbol':
            send({'jsonrpc': '2.0', 'id': 'idle-config', 'method': 'workspace/configuration',
                  'params': {'items': [{'section': 'rust-analyzer'}]}})
