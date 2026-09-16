To **fetch**, given a [request](https://fetch.spec.whatwg.org#concept-request) *request*, an optional algorithm ***processRequestBodyChunkLength***, an optional algorithm ***processRequestEndOfBody***, an optional algorithm ***processEarlyHintsResponse***, an optional algorithm ***processResponse***, an optional algorithm ***processResponseEndOfBody***, an optional algorithm ***processResponseConsumeBody***, and an optional boolean ***useParallelQueue*** (default false), run the steps below. If given, *processRequestBodyChunkLength* must be an algorithm accepting an integer representing the number of bytes transmitted. If given, *processRequestEndOfBody* must be an algorithm accepting no arguments. If given, *processEarlyHintsResponse* must be an algorithm accepting a [response](https://fetch.spec.whatwg.org#concept-response). If given, *processResponse* must be an algorithm accepting a [response](https://fetch.spec.whatwg.org#concept-response). If given, *processResponseEndOfBody* must be an algorithm accepting a [response](https://fetch.spec.whatwg.org#concept-response). If given, *processResponseConsumeBody* must be an algorithm accepting a [response](https://fetch.spec.whatwg.org#concept-response) and null, failure, or a [byte sequence](https://infra.spec.whatwg.org/#byte-sequence).

The user agent may be asked to **suspend** the ongoing fetch. The user agent may either accept or ignore the suspension request. The suspended fetch can be **resumed**. The user agent should ignore the suspension request if the ongoing fetch is updating the response in the HTTP cache for the request.

> **Note:** The user agent does not update the entry in the HTTP cache for a [request](https://fetch.spec.whatwg.org#concept-request) if request’s cache mode is "no-store" or a \``Cache-Control: no-store`\` header appears in the response. \[HTTP-CACHING\]

1. [Assert](https://infra.spec.whatwg.org/#assert): *request*’s [mode](https://fetch.spec.whatwg.org#concept-request-mode) is "`navigate`" or *processEarlyHintsResponse* is null.

    > **Note:** Processing of early hints ([responses](https://fetch.spec.whatwg.org#concept-response) whose [status](https://fetch.spec.whatwg.org#concept-response-status) is 103) is only vetted for navigations.
2. Let *taskDestination* be null.
3. Let *crossOriginIsolatedCapability* be false.
4. [Populate request from client](https://fetch.spec.whatwg.org#populate-request-from-client) given *request*.
5. If *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client) is non-null, then:

    1. Set *taskDestination* to *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client)’s [global object](https://html.spec.whatwg.org/multipage/webappapis.html#concept-settings-object-global).
    2. Set *crossOriginIsolatedCapability* to *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client)’s [cross-origin isolated capability](https://html.spec.whatwg.org/multipage/webappapis.html#concept-settings-object-cross-origin-isolated-capability).
6. If *useParallelQueue* is true, then set *taskDestination* to the result of [starting a new parallel queue](https://html.spec.whatwg.org/multipage/infrastructure.html#starting-a-new-parallel-queue).
7. Let *timingInfo* be a new [fetch timing info](https://fetch.spec.whatwg.org#fetch-timing-info) whose [start time](https://fetch.spec.whatwg.org#fetch-timing-info-start-time) and [post-redirect start time](https://fetch.spec.whatwg.org#fetch-timing-info-post-redirect-start-time) are the [coarsened shared current time](https://w3c.github.io/hr-time/#dfn-coarsened-shared-current-time) given *crossOriginIsolatedCapability*, and [render-blocking](https://fetch.spec.whatwg.org#fetch-timing-info-render-blocking) is set to *request*’s [render-blocking](https://fetch.spec.whatwg.org#request-render-blocking).
8. Let *fetchParams* be a new [fetch params](https://fetch.spec.whatwg.org#fetch-params) whose [request](https://fetch.spec.whatwg.org#fetch-params-request) is *request*, [timing info](https://fetch.spec.whatwg.org#fetch-params-timing-info) is *timingInfo*, [process request body chunk length](https://fetch.spec.whatwg.org#fetch-params-process-request-body) is *processRequestBodyChunkLength*, [process request end-of-body](https://fetch.spec.whatwg.org#fetch-params-process-request-end-of-body) is *processRequestEndOfBody*, [process early hints response](https://fetch.spec.whatwg.org#fetch-params-process-early-hints-response) is *processEarlyHintsResponse*, [process response](https://fetch.spec.whatwg.org#fetch-params-process-response) is *processResponse*, [process response consume body](https://fetch.spec.whatwg.org#fetch-params-process-response-consume-body) is *processResponseConsumeBody*, [process response end-of-body](https://fetch.spec.whatwg.org#fetch-params-process-response-end-of-body) is *processResponseEndOfBody*, [task destination](https://fetch.spec.whatwg.org#fetch-params-task-destination) is *taskDestination*, and [cross-origin isolated capability](https://fetch.spec.whatwg.org#fetch-params-cross-origin-isolated-capability) is *crossOriginIsolatedCapability*.
9. If *request*’s [body](https://fetch.spec.whatwg.org#concept-request-body) is a [byte sequence](https://infra.spec.whatwg.org/#byte-sequence), then set *request*’s [body](https://fetch.spec.whatwg.org#concept-request-body) to *request*’s [body](https://fetch.spec.whatwg.org#concept-request-body) [as a body](https://fetch.spec.whatwg.org#byte-sequence-as-a-body).
10. Run the [WebDriver BiDi clone network request body](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-clone-network-request-body) steps with *request*.
11. If all of the following conditions are true:

    * *request*’s [URL](https://fetch.spec.whatwg.org#concept-request-url)’s [scheme](https://url.spec.whatwg.org/#concept-url-scheme) is an [HTTP(S) scheme](https://fetch.spec.whatwg.org#http-scheme)
    * *request*’s [mode](https://fetch.spec.whatwg.org#concept-request-mode) is "`same-origin`", "`cors`", or "`no-cors`"
    * *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client) is not null, and *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client)’s [global object](https://html.spec.whatwg.org/multipage/webappapis.html#concept-settings-object-global) is a [`Window`](https://html.spec.whatwg.org/multipage/nav-history-apis.html#window) object
    * *request*’s [method](https://fetch.spec.whatwg.org#concept-request-method) is \``GET`\`
    * *request*’s [unsafe-request flag](https://fetch.spec.whatwg.org#unsafe-request-flag) is not set or *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list) [is empty](https://infra.spec.whatwg.org/#list-is-empty)


    then:

    1. [Assert](https://infra.spec.whatwg.org/#assert): *request*’s [origin](https://fetch.spec.whatwg.org#concept-request-origin) is [same origin](https://html.spec.whatwg.org/multipage/browsers.html#same-origin) with *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client)’s [origin](https://html.spec.whatwg.org/multipage/webappapis.html#concept-settings-object-origin).
    2. Let *onPreloadedResponseAvailable* be an algorithm that runs the following step given a [response](https://fetch.spec.whatwg.org#concept-response) *response*: set *fetchParams*’s [preloaded response candidate](https://fetch.spec.whatwg.org#fetch-params-preloaded-response-candidate) to *response*.
    3. Let *foundPreloadedResource* be the result of invoking [consume a preloaded resource](https://html.spec.whatwg.org/multipage/links.html#consume-a-preloaded-resource) for *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client), given *request*’s [URL](https://fetch.spec.whatwg.org#concept-request-url), *request*’s [destination](https://fetch.spec.whatwg.org#concept-request-destination), *request*’s [mode](https://fetch.spec.whatwg.org#concept-request-mode), *request*’s [credentials mode](https://fetch.spec.whatwg.org#concept-request-credentials-mode), *request*’s [integrity metadata](https://fetch.spec.whatwg.org#concept-request-integrity-metadata), and *onPreloadedResponseAvailable*.
    4. If *foundPreloadedResource* is true and *fetchParams*’s [preloaded response candidate](https://fetch.spec.whatwg.org#fetch-params-preloaded-response-candidate) is null, then set *fetchParams*’s [preloaded response candidate](https://fetch.spec.whatwg.org#fetch-params-preloaded-response-candidate) to "`pending`".
12. If *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list) [does not contain](https://fetch.spec.whatwg.org#header-list-contains) \``Accept`\`, then:

    1. Let *value* be \``*/*`\`.
    2. If *request*’s [initiator](https://fetch.spec.whatwg.org#concept-request-initiator) is "`prefetch`", then set *value* to the [document \``Accept`\` header value](https://fetch.spec.whatwg.org#document-accept-header-value).
    3. Otherwise, the user agent should set *value* to the first matching statement, if any, switching on *request*’s [destination](https://fetch.spec.whatwg.org#concept-request-destination):

        "`document`" 

        "`frame`" 

        "`iframe`" 

        the [document \``Accept`\` header value](https://fetch.spec.whatwg.org#document-accept-header-value)

        "`image`" 

        \``image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5`\`

        "`json`" 

        \``application/json,*/*;q=0.5`\`

        "`style`" 

        \``text/css,*/*;q=0.1`\`

        "`text`" 

        \``text/plain,*/*;q=0.5`\`
    4. [Append](https://fetch.spec.whatwg.org#concept-header-list-append) (\``Accept`\`, *value*) to *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list).
13. If *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list) [does not contain](https://fetch.spec.whatwg.org#header-list-contains) \``Accept-Language`\` and *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client) is non-null:

    1. Let *emulatedLanguage* be the [WebDriver BiDi emulated language](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-emulated-language) for *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client).
    2. If *emulatedLanguage* is non-null:

        1. Let *encodedEmulatedLanguage* be *emulatedLanguage*, [isomorphic encoded](https://infra.spec.whatwg.org/#isomorphic-encode).
        2. [Append](https://fetch.spec.whatwg.org#concept-header-list-append) (\``Accept-Language`\`, *encodedEmulatedLanguage*) to *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list).
14. If *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list) [does not contain](https://fetch.spec.whatwg.org#header-list-contains) \``Accept-Language`\`, then user agents should [append](https://fetch.spec.whatwg.org#concept-header-list-append) (\``Accept-Language`, an appropriate [header value](https://fetch.spec.whatwg.org#header-value)) to *request*’s [header list](https://fetch.spec.whatwg.org#concept-request-header-list).
15. If *request*’s [internal priority](https://fetch.spec.whatwg.org#request-internal-priority) is null, then use *request*’s [priority](https://fetch.spec.whatwg.org#request-priority), [initiator](https://fetch.spec.whatwg.org#concept-request-initiator), [destination](https://fetch.spec.whatwg.org#concept-request-destination), and [render-blocking](https://fetch.spec.whatwg.org#request-render-blocking) in an [implementation-defined](https://infra.spec.whatwg.org/#implementation-defined) manner to set *request*’s [internal priority](https://fetch.spec.whatwg.org#request-internal-priority) to an [implementation-defined](https://infra.spec.whatwg.org/#implementation-defined) object.

    > **Note:** The [implementation-defined](https://infra.spec.whatwg.org/#implementation-defined) object could encompass stream weight and dependency for HTTP/2, priorities used in Extensible Prioritization Scheme for HTTP for transports where it applies (including HTTP/3), and equivalent information used to prioritize dispatch and processing of HTTP/1 fetches. \[RFC9218\]
16. If *request* is a [subresource request](https://fetch.spec.whatwg.org#subresource-request):

    1. Let *record* be a new [fetch record](https://fetch.spec.whatwg.org#concept-fetch-record) whose [request](https://fetch.spec.whatwg.org#concept-fetch-record-request) is *request* and [controller](https://fetch.spec.whatwg.org#concept-fetch-record-fetch) is *fetchParams*’s [controller](https://fetch.spec.whatwg.org#fetch-params-controller).
    2. [Append](https://infra.spec.whatwg.org/#list-append) *record* to *request*’s [client](https://fetch.spec.whatwg.org#concept-request-client)’s [fetch group](https://fetch.spec.whatwg.org#environment-settings-object-fetch-group)’s [fetch records](https://fetch.spec.whatwg.org#concept-fetch-record).
17. Run [main fetch](https://fetch.spec.whatwg.org#concept-main-fetch) given *fetchParams*.
18. Return *fetchParams*’s [controller](https://fetch.spec.whatwg.org#fetch-params-controller).
