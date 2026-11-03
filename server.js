var http = require('http');
var dgram = require('dgram');
var fs = require('fs');
var path = require('path');
var os = require('os');
var url = require('url');

var PORT = 9000;
var SSDP_ADDR = '239.255.255.250';
var SSDP_PORT = 1900;
var USB_ROOT = '/tmp/usb';
var SERVER_NAME = 'USB Media Server';
var UDN = 'uuid:e1a2b3c4-d5e6-f7a8-b9c0-d1e2f3a4b5c6';

// DLNA flags: byte-seek enabled, streaming transfer mode
var DLNA_OP = '01'; // 01 = byte seek supported
var DLNA_FLAGS = '01700000000000000000000000000000';

function dlnaFeatures(mime) {
  return 'DLNA.ORG_OP=' + DLNA_OP + ';DLNA.ORG_CI=0;DLNA.ORG_FLAGS=' + DLNA_FLAGS;
}

function dlnaProtocolInfo(mime) {
  return 'http-get:*:' + mime + ':' + dlnaFeatures(mime);
}

var MIME_MAP = {
  mp4: 'video/mp4', mkv: 'video/x-matroska', avi: 'video/x-msvideo',
  mov: 'video/quicktime', webm: 'video/webm', ts: 'video/mp2t',
  flv: 'video/x-flv', wmv: 'video/x-ms-wmv', m4v: 'video/mp4',
  '3gp': 'video/3gpp',
  mp3: 'audio/mpeg', flac: 'audio/flac', aac: 'audio/aac',
  ogg: 'audio/ogg', wav: 'audio/wav', wma: 'audio/x-ms-wma',
  m4a: 'audio/mp4',
  jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png',
  gif: 'image/gif', bmp: 'image/bmp', webp: 'image/webp'
};

var VIDEO_EXT = ['mp4','mkv','avi','mov','webm','ts','flv','wmv','m4v','3gp'];
var AUDIO_EXT = ['mp3','flac','aac','ogg','wav','wma','m4a'];
var IMAGE_EXT = ['jpg','jpeg','png','gif','bmp','webp'];
var SUB_EXT = ['srt','sub','ass','ssa','vtt'];
var SUB_MIME = { srt: 'text/srt', sub: 'text/sub', ass: 'text/x-ass', ssa: 'text/x-ssa', vtt: 'text/vtt' };

function getIP() {
  var ifaces = os.networkInterfaces();
  var keys = Object.keys(ifaces);
  for (var i = 0; i < keys.length; i++) {
    var addrs = ifaces[keys[i]];
    for (var j = 0; j < addrs.length; j++) {
      if (addrs[j].family === 'IPv4' && !addrs[j].internal) return addrs[j].address;
    }
  }
  return '127.0.0.1';
}

var HOST_IP = getIP();
var BASE_URL = 'http://' + HOST_IP + ':' + PORT;

function getExt(name) {
  var i = name.lastIndexOf('.');
  return i > 0 ? name.substring(i + 1).toLowerCase() : '';
}

function getUpnpClass(ext) {
  if (VIDEO_EXT.indexOf(ext) !== -1) return 'object.item.videoItem';
  if (AUDIO_EXT.indexOf(ext) !== -1) return 'object.item.audioItem.musicTrack';
  if (IMAGE_EXT.indexOf(ext) !== -1) return 'object.item.imageItem.photo';
  return null;
}

function escXml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function findUSBRoots() {
  var roots = [];
  try {
    var drives = fs.readdirSync(USB_ROOT);
    for (var i = 0; i < drives.length; i++) {
      var dp = path.join(USB_ROOT, drives[i]);
      try {
        var parts = fs.readdirSync(dp);
        for (var j = 0; j < parts.length; j++) {
          var pp = path.join(dp, parts[j]);
          try {
            var st = fs.statSync(pp);
            if (st.isDirectory()) {
              var contents = fs.readdirSync(pp);
              if (contents.length > 0) roots.push(pp);
            }
          } catch (e) {}
        }
      } catch (e) {}
    }
  } catch (e) {}
  return roots;
}

function findUSBRoot() {
  var roots = findUSBRoots();
  return roots.length > 0 ? roots[0] : null;
}

function resolveIdToPath(id) {
  var roots = findUSBRoots();
  for (var i = 0; i < roots.length; i++) {
    var p = idToPath(id, roots[i]);
    if (p && p.indexOf(roots[i]) === 0) {
      try { fs.statSync(p); return p; } catch (e) {}
    }
  }
  return null;
}

// --- ffprobe metadata ---
var FFPROBE = '/media/developer/dlna-server/ffprobe';
var execSync = require('child_process').execSync;
var metaCache = {};

function probeFile(filePath) {
  if (metaCache[filePath]) return metaCache[filePath];
  var meta = {};
  try {
    execSync(FFPROBE + ' -i ' + JSON.stringify(filePath) + ' 2>&1', { timeout: 5000 });
  } catch (e) {
    // ffmpeg -i always exits non-zero; the info is in stdout
    var s = (e.stdout ? e.stdout.toString() : '') + (e.stderr ? e.stderr.toString() : '');
    if (!s) { metaCache[filePath] = meta; return meta; }
    var dm = s.match(/Duration:\s*(\d+):(\d+):(\d+)\.(\d+)/);
    if (dm) {
      meta.duration = parseInt(dm[1], 10) * 3600 + parseInt(dm[2], 10) * 60 + parseInt(dm[3], 10);
    }
    var vm = s.match(/Stream.*Video.*?(\d{2,5})x(\d{2,5})/);
    if (vm) { meta.width = parseInt(vm[1], 10); meta.height = parseInt(vm[2], 10); }
    var br = s.match(/bitrate:\s*(\d+)\s*kb\/s/);
    if (br) meta.bitrate = parseInt(br[1], 10) * 1000;
  }
  metaCache[filePath] = meta;
  return meta;
}

function formatDuration(seconds) {
  var h = Math.floor(seconds / 3600);
  var m = Math.floor((seconds % 3600) / 60);
  var s = seconds % 60;
  return (h < 10 ? '0' : '') + h + ':' + (m < 10 ? '0' : '') + m + ':' + (s < 10 ? '0' : '') + s + '.000';
}

function findSubtitles(videoPath) {
  var dir = path.dirname(videoPath);
  var base = path.basename(videoPath, path.extname(videoPath));
  var subs = [];
  try {
    var files = fs.readdirSync(dir);
    for (var i = 0; i < files.length; i++) {
      var ext = getExt(files[i]);
      if (SUB_EXT.indexOf(ext) === -1) continue;
      var subBase = path.basename(files[i], '.' + ext);
      // Match: same name, or same name + language tag (e.g. Movie.en.srt)
      if (subBase === base || subBase.indexOf(base) === 0) {
        var subPath = path.join(dir, files[i]);
        subs.push({
          path: subPath,
          name: files[i],
          ext: ext,
          mime: SUB_MIME[ext] || 'text/plain'
        });
      }
    }
  } catch (e) {}
  return subs;
}

// Encode a filesystem path as an object ID: base64 of relative path from USB root
function pathToId(absPath, usbRoot) {
  var rel = absPath.substring(usbRoot.length);
  if (!rel) rel = '/';
  return Buffer.from(rel).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function idToPath(id, usbRoot) {
  if (id === '0') return usbRoot;
  var b64 = id.replace(/-/g, '+').replace(/_/g, '/');
  while (b64.length % 4) b64 += '=';
  var rel = Buffer.from(b64, 'base64').toString('utf8');
  var resolved = path.resolve(usbRoot, '.' + rel);
  if (resolved.indexOf(usbRoot) !== 0) return null;
  return resolved;
}

function browseDirectChildren(objectID, start, count, usbRoot) {
  // Root browse: list all USB partitions as top-level folders
  if (objectID === '0') {
    var roots = findUSBRoots();
    var items = [];
    for (var r = 0; r < roots.length; r++) {
      var rp = roots[r];
      var label = path.basename(rp);
      // Use a partition-prefixed ID so we know which root it belongs to
      var id = 'root:' + label;
      items.push({
        id: id, parentId: '0', title: label,
        isDir: true, upnpClass: 'object.container.storageFolder'
      });
    }
    var total = items.length;
    if (count > 0) items = items.slice(start, start + count);
    return { items: items, total: total };
  }

  // Resolve the right USB root from the object ID
  var effectiveRoot = usbRoot;
  var effectiveID = objectID;
  if (objectID.indexOf('root:') === 0) {
    var partName = objectID.substring(5);
    var roots2 = findUSBRoots();
    for (var r2 = 0; r2 < roots2.length; r2++) {
      if (path.basename(roots2[r2]) === partName) {
        effectiveRoot = roots2[r2];
        effectiveID = '0';
        break;
      }
    }
  } else {
    // Find which root this ID belongs to by trying each
    var roots3 = findUSBRoots();
    for (var r3 = 0; r3 < roots3.length; r3++) {
      var resolved = idToPath(objectID, roots3[r3]);
      if (resolved && resolved.indexOf(roots3[r3]) === 0) {
        try {
          fs.statSync(resolved);
          effectiveRoot = roots3[r3];
          break;
        } catch (e) {}
      }
    }
  }

  var dirPath = effectiveID === '0' ? effectiveRoot : idToPath(effectiveID, effectiveRoot);
  if (!dirPath) return { items: [], total: 0 };

  var entries;
  try { entries = fs.readdirSync(dirPath); } catch (e) { return { items: [], total: 0 }; }

  var items = [];
  for (var i = 0; i < entries.length; i++) {
    if (entries[i].charAt(0) === '.') continue;
    var fullPath = path.join(dirPath, entries[i]);
    var stat;
    var statOk = true;
    try { stat = fs.statSync(fullPath); } catch (e) { stat = null; statOk = false; }

    var id = pathToId(fullPath, effectiveRoot);
    var parentId = objectID;

    if (statOk && stat.isDirectory()) {
      items.push({
        id: id, parentId: parentId, title: entries[i],
        isDir: true, upnpClass: 'object.container.storageFolder'
      });
    } else {
      var ext = getExt(entries[i]);
      var upnpClass = getUpnpClass(ext);
      if (!upnpClass) continue;
      var mime = MIME_MAP[ext] || 'application/octet-stream';
      var fileSize = (statOk && stat) ? stat.size : 0;
      var item = {
        id: id, parentId: parentId, title: entries[i],
        isDir: false, upnpClass: upnpClass,
        mime: mime, size: fileSize,
        url: BASE_URL + '/media/' + encodeURIComponent(id)
      };
      if (VIDEO_EXT.indexOf(ext) !== -1 || AUDIO_EXT.indexOf(ext) !== -1) {
        var meta = probeFile(fullPath);
        if (meta.duration) item.duration = meta.duration;
        if (meta.width) { item.width = meta.width; item.height = meta.height; }
        if (meta.bitrate) item.bitrate = meta.bitrate;
      }
      if (VIDEO_EXT.indexOf(ext) !== -1) {
        var subs = findSubtitles(fullPath);
        if (subs.length) {
          item.subs = subs.map(function (s) {
            return {
              url: BASE_URL + '/sub/' + encodeURIComponent(pathToId(s.path, effectiveRoot)),
              ext: s.ext,
              mime: s.mime,
              name: s.name
            };
          });
        }
      }
      items.push(item);
    }
  }

  items.sort(function (a, b) {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    return a.title.toLowerCase().localeCompare(b.title.toLowerCase());
  });

  var total = items.length;
  var slice = items.slice(start, count > 0 ? start + count : undefined);
  return { items: slice, total: total };
}

function didlResult(items) {
  var xml = '<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
    + 'xmlns:dc="http://purl.org/dc/elements/1.1/" '
    + 'xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" '
    + 'xmlns:dlna="urn:schemas-dlna-org:metadata-1-0/" '
    + 'xmlns:sec="http://www.sec.co.kr/" '
    + 'xmlns:pv="http://www.pv.com/pvns/">';

  for (var i = 0; i < items.length; i++) {
    var it = items[i];
    if (it.isDir) {
      xml += '<container id="' + escXml(it.id) + '" parentID="' + escXml(it.parentId)
        + '" restricted="1" searchable="0">'
        + '<dc:title>' + escXml(it.title) + '</dc:title>'
        + '<upnp:class>' + it.upnpClass + '</upnp:class>'
        + '</container>';
    } else {
      var resAttrs = 'protocolInfo="' + dlnaProtocolInfo(it.mime) + '" size="' + it.size + '"';
      if (it.duration) resAttrs += ' duration="' + formatDuration(it.duration) + '"';
      if (it.width) resAttrs += ' resolution="' + it.width + 'x' + it.height + '"';
      if (it.bitrate) resAttrs += ' bitrate="' + it.bitrate + '"';
      xml += '<item id="' + escXml(it.id) + '" parentID="' + escXml(it.parentId)
        + '" restricted="1">'
        + '<dc:title>' + escXml(it.title) + '</dc:title>'
        + '<upnp:class>' + it.upnpClass + '</upnp:class>'
        + '<res ' + resAttrs + '>'
        + escXml(it.url) + '</res>';
      if (it.subs) {
        for (var si = 0; si < it.subs.length; si++) {
          var sub = it.subs[si];
          xml += '<sec:CaptionInfoEx sec:type="' + sub.ext + '">'
            + escXml(sub.url) + '</sec:CaptionInfoEx>';
          xml += '<res protocolInfo="http-get:*:' + sub.mime + ':*">'
            + escXml(sub.url) + '</res>';
        }
      }
      xml += '</item>';
    }
  }
  xml += '</DIDL-Lite>';
  return xml;
}

function deviceXml() {
  return '<?xml version="1.0" encoding="UTF-8"?>'
    + '<root xmlns="urn:schemas-upnp-org:device-1-0">'
    + '<specVersion><major>1</major><minor>0</minor></specVersion>'
    + '<device>'
    + '<deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>'
    + '<friendlyName>' + escXml(SERVER_NAME) + '</friendlyName>'
    + '<manufacturer>Homebrew</manufacturer>'
    + '<modelName>NodeDLNA</modelName>'
    + '<modelNumber>1.0</modelNumber>'
    + '<UDN>' + UDN + '</UDN>'
    + '<serviceList>'
    + '<service>'
    + '<serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>'
    + '<serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>'
    + '<SCPDURL>/cds.xml</SCPDURL>'
    + '<controlURL>/cds/control</controlURL>'
    + '<eventSubURL>/cds/event</eventSubURL>'
    + '</service>'
    + '<service>'
    + '<serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>'
    + '<serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>'
    + '<SCPDURL>/cms.xml</SCPDURL>'
    + '<controlURL>/cms/control</controlURL>'
    + '<eventSubURL>/cms/event</eventSubURL>'
    + '</service>'
    + '</serviceList>'
    + '</device>'
    + '</root>';
}

function cdsXml() {
  return '<?xml version="1.0" encoding="UTF-8"?>'
    + '<scpd xmlns="urn:schemas-upnp-org:service-1-0">'
    + '<specVersion><major>1</major><minor>0</minor></specVersion>'
    + '<actionList>'
    + '<action><name>Browse</name><argumentList>'
    + '<argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>'
    + '<argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>'
    + '<argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>'
    + '<argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>'
    + '<argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>'
    + '<argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>'
    + '<argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>'
    + '<argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>'
    + '<argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>'
    + '<argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '<action><name>GetSystemUpdateID</name><argumentList>'
    + '<argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '<action><name>GetSearchCapabilities</name><argumentList>'
    + '<argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '<action><name>GetSortCapabilities</name><argumentList>'
    + '<argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '</actionList>'
    + '<serviceStateTable>'
    + '<stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType><allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>'
    + '</serviceStateTable>'
    + '</scpd>';
}

function cmsXml() {
  return '<?xml version="1.0" encoding="UTF-8"?>'
    + '<scpd xmlns="urn:schemas-upnp-org:service-1-0">'
    + '<specVersion><major>1</major><minor>0</minor></specVersion>'
    + '<actionList>'
    + '<action><name>GetProtocolInfo</name><argumentList>'
    + '<argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>'
    + '<argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '<action><name>GetCurrentConnectionIDs</name><argumentList>'
    + '<argument><name>ConnectionIDs</name><direction>out</direction><relatedStateVariable>CurrentConnectionIDs</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '<action><name>GetCurrentConnectionInfo</name><argumentList>'
    + '<argument><name>ConnectionID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>'
    + '<argument><name>RcsID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_RcsID</relatedStateVariable></argument>'
    + '<argument><name>AVTransportID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_AVTransportID</relatedStateVariable></argument>'
    + '<argument><name>ProtocolInfo</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ProtocolInfo</relatedStateVariable></argument>'
    + '<argument><name>PeerConnectionManager</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionManager</relatedStateVariable></argument>'
    + '<argument><name>PeerConnectionID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>'
    + '<argument><name>Direction</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Direction</relatedStateVariable></argument>'
    + '<argument><name>Status</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionStatus</relatedStateVariable></argument>'
    + '</argumentList></action>'
    + '</actionList>'
    + '<serviceStateTable>'
    + '<stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="yes"><name>CurrentConnectionIDs</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionStatus</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionManager</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_Direction</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_ProtocolInfo</name><dataType>string</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionID</name><dataType>i4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_AVTransportID</name><dataType>i4</dataType></stateVariable>'
    + '<stateVariable sendEvents="no"><name>A_ARG_TYPE_RcsID</name><dataType>i4</dataType></stateVariable>'
    + '</serviceStateTable>'
    + '</scpd>';
}

function parseSoapAction(body) {
  var m;
  var args = {};
  m = body.match(/<ObjectID>([^<]*)<\/ObjectID>/);
  if (m) args.ObjectID = m[1].replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
  m = body.match(/<BrowseFlag>([^<]*)<\/BrowseFlag>/);
  if (m) args.BrowseFlag = m[1];
  m = body.match(/<StartingIndex>([^<]*)<\/StartingIndex>/);
  if (m) args.StartingIndex = parseInt(m[1], 10) || 0;
  m = body.match(/<RequestedCount>([^<]*)<\/RequestedCount>/);
  if (m) args.RequestedCount = parseInt(m[1], 10) || 0;
  return args;
}

function soapResponse(actionName, params) {
  var xml = '<?xml version="1.0" encoding="UTF-8"?>'
    + '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" '
    + 's:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">'
    + '<s:Body>'
    + '<u:' + actionName + 'Response xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">';
  var keys = Object.keys(params);
  for (var i = 0; i < keys.length; i++) {
    xml += '<' + keys[i] + '>' + escXml(String(params[keys[i]])) + '</' + keys[i] + '>';
  }
  xml += '</u:' + actionName + 'Response></s:Body></s:Envelope>';
  return xml;
}

function cmsSoapResponse(actionName, params) {
  var xml = '<?xml version="1.0" encoding="UTF-8"?>'
    + '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" '
    + 's:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">'
    + '<s:Body>'
    + '<u:' + actionName + 'Response xmlns:u="urn:schemas-upnp-org:service:ConnectionManager:1">';
  var keys = Object.keys(params);
  for (var i = 0; i < keys.length; i++) {
    xml += '<' + keys[i] + '>' + escXml(String(params[keys[i]])) + '</' + keys[i] + '>';
  }
  xml += '</u:' + actionName + 'Response></s:Body></s:Envelope>';
  return xml;
}

// --- SSDP ---
function startSSDP() {
  var sock = dgram.createSocket({ type: 'udp4', reuseAddr: true });

  sock.on('listening', function () {
    sock.addMembership(SSDP_ADDR);
    console.log('SSDP listening on ' + SSDP_ADDR + ':' + SSDP_PORT);
    advertise();
    setInterval(advertise, 30000);
  });

  sock.on('message', function (msg, rinfo) {
    var s = msg.toString();
    if (s.indexOf('M-SEARCH') !== 0) return;
    if (s.indexOf('ssdp:all') !== -1
      || s.indexOf('upnp:rootdevice') !== -1
      || s.indexOf('urn:schemas-upnp-org:device:MediaServer') !== -1
      || s.indexOf('urn:schemas-upnp-org:service:ContentDirectory') !== -1) {
      var resp = 'HTTP/1.1 200 OK\r\n'
        + 'CACHE-CONTROL: max-age=1800\r\n'
        + 'LOCATION: ' + BASE_URL + '/device.xml\r\n'
        + 'SERVER: webOS/5.0 UPnP/1.0 NodeDLNA/1.0\r\n'
        + 'ST: urn:schemas-upnp-org:device:MediaServer:1\r\n'
        + 'USN: ' + UDN + '::urn:schemas-upnp-org:device:MediaServer:1\r\n'
        + 'EXT:\r\n'
        + '\r\n';
      var buf = Buffer.from(resp);
      sock.send(buf, 0, buf.length, rinfo.port, rinfo.address);
    }
  });

  sock.bind(SSDP_PORT);

  function advertise() {
    var types = [
      'upnp:rootdevice',
      UDN,
      'urn:schemas-upnp-org:device:MediaServer:1',
      'urn:schemas-upnp-org:service:ContentDirectory:1',
      'urn:schemas-upnp-org:service:ConnectionManager:1'
    ];
    for (var i = 0; i < types.length; i++) {
      var msg = 'NOTIFY * HTTP/1.1\r\n'
        + 'HOST: ' + SSDP_ADDR + ':' + SSDP_PORT + '\r\n'
        + 'CACHE-CONTROL: max-age=1800\r\n'
        + 'LOCATION: ' + BASE_URL + '/device.xml\r\n'
        + 'NT: ' + types[i] + '\r\n'
        + 'NTS: ssdp:alive\r\n'
        + 'SERVER: webOS/5.0 UPnP/1.0 NodeDLNA/1.0\r\n'
        + 'USN: ' + UDN + (types[i] === UDN ? '' : '::' + types[i]) + '\r\n'
        + '\r\n';
      var buf = Buffer.from(msg);
      sock.send(buf, 0, buf.length, SSDP_PORT, SSDP_ADDR);
    }
  }

  return sock;
}

// --- HTTP Server ---
var server = http.createServer(function (req, res) {
  var parsed = url.parse(req.url, true);
  var pathname = parsed.pathname;

  console.log(req.method + ' ' + pathname);

  if (pathname === '/device.xml') {
    res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
    res.end(deviceXml());
    return;
  }

  if (pathname === '/cds.xml') {
    res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
    res.end(cdsXml());
    return;
  }

  if (pathname === '/cms.xml') {
    res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
    res.end(cmsXml());
    return;
  }

  if (pathname === '/cds/control' && req.method === 'POST') {
    var body = '';
    req.on('data', function (chunk) { body += chunk; });
    req.on('end', function () {
      var usbRoot = findUSBRoot();
      if (!usbRoot) {
        res.writeHead(500);
        res.end('No USB');
        return;
      }

      var soapAction = req.headers['soapaction'] || '';

      if (soapAction.indexOf('Browse') !== -1) {
        var args = parseSoapAction(body);
        var objectID = args.ObjectID || '0';
        var startIdx = args.StartingIndex || 0;
        var reqCount = args.RequestedCount || 0;
        console.log('Browse: ' + args.BrowseFlag + ' id=' + objectID);

        if (args.BrowseFlag === 'BrowseMetadata') {
          var isRoot = objectID === '0';
          var title;
          if (isRoot) {
            title = SERVER_NAME;
          } else if (objectID.indexOf('root:') === 0) {
            title = objectID.substring(5);
          } else {
            var metaPath = resolveIdToPath(objectID);
            title = metaPath ? path.basename(metaPath) : objectID;
          }
          var didl = '<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" '
            + 'xmlns:dc="http://purl.org/dc/elements/1.1/" '
            + 'xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">'
            + '<container id="' + escXml(objectID) + '" parentID="-1" restricted="1">'
            + '<dc:title>' + escXml(title) + '</dc:title>'
            + '<upnp:class>object.container.storageFolder</upnp:class>'
            + '</container></DIDL-Lite>';
          var respXml = soapResponse('Browse', {
            Result: didl, NumberReturned: 1, TotalMatches: 1, UpdateID: 1
          });
          res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
          res.end(respXml);
          return;
        }

        var result = browseDirectChildren(objectID, startIdx, reqCount, usbRoot);
        var didlXml = didlResult(result.items);
        var respBody = soapResponse('Browse', {
          Result: didlXml,
          NumberReturned: result.items.length,
          TotalMatches: result.total,
          UpdateID: 1
        });
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(respBody);
      } else if (soapAction.indexOf('GetSearchCapabilities') !== -1) {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(soapResponse('GetSearchCapabilities', { SearchCaps: '' }));
      } else if (soapAction.indexOf('GetSystemUpdateID') !== -1) {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(soapResponse('GetSystemUpdateID', { Id: 1 }));
      } else if (soapAction.indexOf('GetSortCapabilities') !== -1) {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(soapResponse('GetSortCapabilities', { SortCaps: '' }));
      } else {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end('<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body></s:Body></s:Envelope>');
      }
    });
    return;
  }

  if (pathname === '/cms/control' && req.method === 'POST') {
    var body2 = '';
    req.on('data', function (chunk) { body2 += chunk; });
    req.on('end', function () {
      var soapAction2 = req.headers['soapaction'] || '';
      if (soapAction2.indexOf('GetProtocolInfo') !== -1) {
        var df = 'DLNA.ORG_OP=' + DLNA_OP + ';DLNA.ORG_CI=0;DLNA.ORG_FLAGS=' + DLNA_FLAGS;
        var protocols = 'http-get:*:video/mp4:' + df
          + ',http-get:*:video/x-matroska:' + df
          + ',http-get:*:video/x-msvideo:' + df
          + ',http-get:*:video/quicktime:' + df
          + ',http-get:*:audio/mpeg:' + df
          + ',http-get:*:audio/flac:' + df
          + ',http-get:*:image/jpeg:' + df
          + ',http-get:*:image/png:' + df;
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(cmsSoapResponse('GetProtocolInfo', { Source: protocols, Sink: '' }));
      } else if (soapAction2.indexOf('GetCurrentConnectionIDs') !== -1) {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(cmsSoapResponse('GetCurrentConnectionIDs', { ConnectionIDs: '0' }));
      } else {
        res.writeHead(200, { 'Content-Type': 'text/xml; charset="utf-8"' });
        res.end(cmsSoapResponse('GetCurrentConnectionInfo', {
          RcsID: -1, AVTransportID: -1,
          ProtocolInfo: '', PeerConnectionManager: '',
          PeerConnectionID: -1, Direction: 'Output', Status: 'OK'
        }));
      }
    });
    return;
  }

  // Event subscriptions (minimal)
  if ((pathname === '/cds/event' || pathname === '/cms/event') && req.method === 'SUBSCRIBE') {
    res.writeHead(200, {
      'SID': 'uuid:sub-' + Date.now(),
      'TIMEOUT': 'Second-1800'
    });
    res.end();
    return;
  }

  // Subtitle file serving
  if (pathname.indexOf('/sub/') === 0) {
    var subId = decodeURIComponent(pathname.substring(5));
    var subPath = resolveIdToPath(subId);
    if (!subPath) { res.writeHead(404); res.end('Not found'); return; }
    try {
      var subData = fs.readFileSync(subPath);
      var subExt = getExt(subPath);
      res.writeHead(200, {
        'Content-Type': SUB_MIME[subExt] || 'text/plain',
        'Content-Length': subData.length,
        'Access-Control-Allow-Origin': '*'
      });
      res.end(subData);
    } catch (e) {
      res.writeHead(404); res.end('Not found');
    }
    return;
  }

  // Media file streaming
  if (pathname.indexOf('/media/') === 0) {
    var fileId = decodeURIComponent(pathname.substring(7));
    var filePath = resolveIdToPath(fileId);
    if (!filePath) {
      res.writeHead(404);
      res.end('Not found');
      return;
    }

    var fileSize;
    try {
      var sizeOut = execSync('stat -c %s ' + JSON.stringify(filePath), { timeout: 3000 });
      fileSize = parseInt(sizeOut.toString().trim(), 10);
    } catch (e) {
      try { fileSize = fs.statSync(filePath).size; } catch (e2) {
        res.writeHead(404);
        res.end('Not found');
        return;
      }
    }

    var ext2 = getExt(filePath);
    var mime2 = MIME_MAP[ext2] || 'application/octet-stream';

    // Range request support
    var range = req.headers.range;
    var features = dlnaFeatures(mime2);
    if (range) {
      var parts = range.replace(/bytes=/, '').split('-');
      var rangeStart = parseInt(parts[0], 10);
      var rangeEnd = parts[1] ? parseInt(parts[1], 10) : fileSize - 1;
      if (rangeEnd >= fileSize) rangeEnd = fileSize - 1;
      var chunkSize = rangeEnd - rangeStart + 1;
      var stream = fs.createReadStream(filePath, { start: rangeStart, end: rangeEnd });
      res.writeHead(206, {
        'Content-Range': 'bytes ' + rangeStart + '-' + rangeEnd + '/' + fileSize,
        'Accept-Ranges': 'bytes',
        'Content-Length': chunkSize,
        'Content-Type': mime2,
        'transferMode.dlna.org': 'Streaming',
        'contentFeatures.dlna.org': features
      });
      stream.pipe(res);
    } else {
      res.writeHead(200, {
        'Content-Length': fileSize,
        'Content-Type': mime2,
        'Accept-Ranges': 'bytes',
        'transferMode.dlna.org': 'Streaming',
        'contentFeatures.dlna.org': features
      });
      fs.createReadStream(filePath).pipe(res);
    }
    return;
  }

  res.writeHead(404);
  res.end('Not found');
});

server.listen(PORT, function () {
  var usbRoot = findUSBRoot();
  console.log('DLNA media server running on ' + BASE_URL);
  console.log('USB root: ' + (usbRoot || 'NOT FOUND'));
  console.log('Host IP: ' + HOST_IP);
  startSSDP();
});
