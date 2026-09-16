To **navigate** a [navigable](https://html.spec.whatwg.org#navigable) *navigable* to a [URL](https://url.spec.whatwg.org/#concept-url) *url* using an optional [`Document`](https://html.spec.whatwg.org#document)-or-null *sourceDocument* (default null), with an optional [POST resource](https://html.spec.whatwg.org#post-resource), string, or null ***documentResource*** (default null), an optional [response](https://fetch.spec.whatwg.org/#concept-response)-or-null ***response*** (default null), an optional boolean ***exceptionsEnabled*** (default false), an optional [`NavigationHistoryBehavior`](https://html.spec.whatwg.org#navigationhistorybehavior) ***historyHandling*** (default "[`auto`](https://html.spec.whatwg.org#navigationhistorybehavior-auto)"), an optional [serialized state](https://html.spec.whatwg.org#serialized-state)-or-null ***navigationAPIState*** (default null), an optional [entry list](https://html.spec.whatwg.org#entry-list) or null ***formDataEntryList*** (default null), an optional [referrer policy](https://w3c.github.io/webappsec-referrer-policy/#referrer-policy) ***referrerPolicy*** (default the empty string), an optional [user navigation involvement](https://html.spec.whatwg.org#user-navigation-involvement) ***userInvolvement*** (default "[`none`](https://html.spec.whatwg.org#uni-none)"), an optional [`Element`](https://dom.spec.whatwg.org/#interface-element) ***sourceElement*** (default null), an optional boolean ***initialInsertion*** (default false), and an optional [navigation API method tracker](https://html.spec.whatwg.org#navigation-api-method-tracker)-or-null ***apiMethodTracker*** (default null):

1. Let *cspNavigationType* be "`form-submission`" if *formDataEntryList* is non-null; otherwise "`other`".
2. Let *sourceSnapshotParams* be the result of [snapshotting source snapshot params](https://html.spec.whatwg.org#snapshotting-source-snapshot-params) given *sourceDocument*.
3. Let *initiatorOriginSnapshot* be a new [opaque origin](https://html.spec.whatwg.org#concept-origin-opaque).
4. Let *initiatorBaseURLSnapshot* be [`about:blank`](https://html.spec.whatwg.org#about:blank).
5. If *sourceDocument* is null:

    1. [Assert](https://infra.spec.whatwg.org/#assert): *userInvolvement* is "[`browser UI`](https://html.spec.whatwg.org#uni-browser-ui)".
    2. If *url*'s [scheme](https://url.spec.whatwg.org/#concept-url-scheme) is "[`javascript`](https://html.spec.whatwg.org#the-javascript:-url-special-case)", then set *initiatorOriginSnapshot* to *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [origin](https://dom.spec.whatwg.org/#concept-document-origin).
6. Otherwise:

    1. [Assert](https://infra.spec.whatwg.org/#assert): *userInvolvement* is not "[`browser UI`](https://html.spec.whatwg.org#uni-browser-ui)".
    2. If *sourceDocument*'s [node navigable](https://html.spec.whatwg.org#node-navigable) is not [allowed by sandboxing to navigate](https://html.spec.whatwg.org#allowed-to-navigate) *navigable* given *sourceSnapshotParams*:

        1. If *exceptionsEnabled* is true, then throw a ["`SecurityError`"](https://webidl.spec.whatwg.org/#securityerror) [`DOMException`](https://webidl.spec.whatwg.org/#dfn-DOMException).
        2. Return.
    3. Set *initiatorOriginSnapshot* to *sourceDocument*'s [origin](https://dom.spec.whatwg.org/#concept-document-origin).
    4. Set *initiatorBaseURLSnapshot* to *sourceDocument*'s [document base URL](https://html.spec.whatwg.org#document-base-url).
7. Let *navigationId* be the result of [generating a random UUID](https://w3c.github.io/webcrypto/#dfn-generate-a-random-uuid). [\[WEBCRYPTO\]](https://html.spec.whatwg.org#refsWEBCRYPTO)
8. If the [surrounding agent](https://tc39.es/ecma262/#surrounding-agent) is equal to *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [relevant agent](https://html.spec.whatwg.org#relevant-agent), then continue these steps. Otherwise, [queue a global task](https://html.spec.whatwg.org#queue-a-global-task) on the [navigation and traversal task source](https://html.spec.whatwg.org#navigation-and-traversal-task-source) given *navigable*'s [active window](https://html.spec.whatwg.org#nav-window) to continue these steps.

    > **Note:** We do this because we are about to look at a lot of properties of *navigable*'s [active document](https://html.spec.whatwg.org#nav-document), which are in theory only accessible over in the appropriate [event loop](https://html.spec.whatwg.org#event-loop). (But, we do not want to unconditionally queue a task, since — for example — same-event-loop [fragment navigations](https://html.spec.whatwg.org#navigate-fragid) need to take effect synchronously.)
    >
    > Another implementation strategy would be to replicate the relevant information across event loops, or into a canonical "browser process", so that it can be consulted without queueing a task. This could give different results than what we specify here in edge cases, where the relevant properties have changed over in the target event loop but not yet been replicated. Further testing is needed to determine which of these strategies best matches browser behavior, in such racy edge cases.
9. If *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [unload counter](https://html.spec.whatwg.org#unload-counter) is greater than 0, then invoke [WebDriver BiDi navigation failed](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-failed) with *navigable* and a [WebDriver BiDi navigation status](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-status) whose [id](https://w3c.github.io/webdriver-bidi/#navigation-status-id) is *navigationId*, [status](https://w3c.github.io/webdriver-bidi/#navigation-status-status) is "[`canceled`](https://w3c.github.io/webdriver-bidi/#navigation-status-canceled)", and [url](https://w3c.github.io/webdriver-bidi/#navigation-status-url) is *url*, and return.
10. Let *container* be *navigable*'s [container](https://html.spec.whatwg.org#nav-container).
11. If *container* is an [`iframe`](https://html.spec.whatwg.org#the-iframe-element) element and [will lazy load element steps](https://html.spec.whatwg.org#will-lazy-load-element-steps) given *container* returns true, then [stop intersection-observing a lazy loading element](https://html.spec.whatwg.org#stop-intersection-observing-a-lazy-loading-element) *container* and set *container*'s [lazy load resumption steps](https://html.spec.whatwg.org#lazy-load-resumption-steps) to null.
12. If *navigable*'s [allowed to perform a navigation or history update](https://html.spec.whatwg.org#allowed-to-perform-a-navigation-or-history-update) returns [blocked](https://infra.spec.whatwg.org/#blocked), then invoke [WebDriver BiDi navigation failed](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-failed) with *navigable* and a [WebDriver BiDi navigation status](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-status) whose [id](https://w3c.github.io/webdriver-bidi/#navigation-status-id) is *navigationId*, [status](https://w3c.github.io/webdriver-bidi/#navigation-status-status) is "[`canceled`](https://w3c.github.io/webdriver-bidi/#navigation-status-canceled)", and [url](https://w3c.github.io/webdriver-bidi/#navigation-status-url) is *url*, and return.
13. If *historyHandling* is "[`auto`](https://html.spec.whatwg.org#navigationhistorybehavior-auto)":

    1. If *url* [equals](https://url.spec.whatwg.org/#concept-url-equals) *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [URL](https://dom.spec.whatwg.org/#concept-document-url), and either *userInvolvement* is "[`browser UI`](https://html.spec.whatwg.org#uni-browser-ui)" or *initiatorOriginSnapshot* is [same origin](https://html.spec.whatwg.org#same-origin) with *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [origin](https://dom.spec.whatwg.org/#concept-document-origin), then set *historyHandling* to "[`replace`](https://html.spec.whatwg.org#navigationhistorybehavior-replace)".
    2. Otherwise, set *historyHandling* to "[`push`](https://html.spec.whatwg.org#navigationhistorybehavior-push)".
14. If [the navigation must be a replace](https://html.spec.whatwg.org#the-navigation-must-be-a-replace) given *url* and *navigable*'s [active document](https://html.spec.whatwg.org#nav-document), then set *historyHandling* to "[`replace`](https://html.spec.whatwg.org#navigationhistorybehavior-replace)".
15. If all of the following are true:

    * *documentResource* is null;
    * *response* is null;
    * *url* [equals](https://url.spec.whatwg.org/#concept-url-equals) *navigable*'s [active session history entry](https://html.spec.whatwg.org#nav-active-history-entry)'s [URL](https://html.spec.whatwg.org#she-url) with [*exclude fragments*](https://url.spec.whatwg.org/#url-equals-exclude-fragments) set to true; and
    * *url*'s [fragment](https://url.spec.whatwg.org/#concept-url-fragment) is non-null,


    then:

    1. [Navigate to a fragment](https://html.spec.whatwg.org#navigate-fragid) given *navigable*, *url*, *historyHandling*, *userInvolvement*, *sourceElement*, *navigationAPIState*, and *navigationId*.
    2. Return.
16. If *navigable*'s [parent](https://html.spec.whatwg.org#nav-parent) is non-null, then set *navigable*'s [is delaying `load` events](https://html.spec.whatwg.org#delaying-load-events-mode) to true.
17. Let *targetSnapshotParams* be the result of [snapshotting target snapshot params](https://html.spec.whatwg.org#snapshotting-target-snapshot-params) given *navigable*.
18. Invoke [WebDriver BiDi navigation started](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-started) with *navigable* and a new [WebDriver BiDi navigation status](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-status) whose [id](https://w3c.github.io/webdriver-bidi/#navigation-status-id) is *navigationId*, [status](https://w3c.github.io/webdriver-bidi/#navigation-status-status) is "[`pending`](https://w3c.github.io/webdriver-bidi/#navigation-status-pending)", and [url](https://w3c.github.io/webdriver-bidi/#navigation-status-url) is *url*.
19. If *navigable*'s [ongoing navigation](https://html.spec.whatwg.org#ongoing-navigation) is "`traversal`":

    1. Invoke [WebDriver BiDi navigation failed](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-failed) with *navigable* and a new [WebDriver BiDi navigation status](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-status) whose [id](https://w3c.github.io/webdriver-bidi/#navigation-status-id) is *navigationId*, [status](https://w3c.github.io/webdriver-bidi/#navigation-status-status) is "[`canceled`](https://w3c.github.io/webdriver-bidi/#navigation-status-canceled)", and [url](https://w3c.github.io/webdriver-bidi/#navigation-status-url) is *url*.
    2. Return.


    > **Note:** Any attempts to navigate a [navigable](https://html.spec.whatwg.org#navigable) that is currently [traversing](https://html.spec.whatwg.org#apply-the-traverse-history-step) are ignored.
20. [Set the ongoing navigation](https://html.spec.whatwg.org#set-the-ongoing-navigation) for *navigable* to *navigationId*.

    > **Note:** This will have the effect of aborting other ongoing navigations of *navigable*, since at certain points during navigation changes to the [ongoing navigation](https://html.spec.whatwg.org#ongoing-navigation) will cause further work to be abandoned.
21. If *url*'s [scheme](https://url.spec.whatwg.org/#concept-url-scheme) is "[`javascript`](https://html.spec.whatwg.org#the-javascript:-url-special-case)":

    1. [Queue a global task](https://html.spec.whatwg.org#queue-a-global-task) on the [navigation and traversal task source](https://html.spec.whatwg.org#navigation-and-traversal-task-source) given *navigable*'s [active window](https://html.spec.whatwg.org#nav-window) to [navigate to a `javascript:` URL](https://html.spec.whatwg.org#navigate-to-a-javascript:-url) given *navigable*, *url*, *historyHandling*, *sourceSnapshotParams*, *initiatorOriginSnapshot*, *userInvolvement*, *cspNavigationType*, *initialInsertion*, and *navigationId*.
    2. Return.
22. If all of the following are true:

    * *userInvolvement* is not "[`browser
         UI`](https://html.spec.whatwg.org#uni-browser-ui)";
    * *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [origin](https://dom.spec.whatwg.org/#concept-document-origin) is [same origin-domain](https://html.spec.whatwg.org#same-origin-domain) with *sourceDocument*'s [origin](https://dom.spec.whatwg.org/#concept-document-origin);
    * *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [is initial `about:blank`](https://html.spec.whatwg.org#is-initial-about:blank) is false; and
    * *url*'s [scheme](https://url.spec.whatwg.org/#concept-url-scheme) is a [fetch scheme](https://fetch.spec.whatwg.org/#fetch-scheme),


    then:

    1. Let *navigation* be *navigable*'s [active window](https://html.spec.whatwg.org#nav-window)'s [navigation API](https://html.spec.whatwg.org#window-navigation-api).
    2. Let *entryListForFiring* be *formDataEntryList* if *documentResource* is a [POST resource](https://html.spec.whatwg.org#post-resource); otherwise, null.
    3. Let *navigationAPIStateForFiring* be *navigationAPIState* if *navigationAPIState* is not null; otherwise, [StructuredSerializeForStorage](https://html.spec.whatwg.org#structuredserializeforstorage)(undefined).
    4. Let *continue* be the result of [firing a push/replace/reload `navigate` event](https://html.spec.whatwg.org#fire-a-push/replace/reload-navigate-event) at *navigation* with *[navigationType](https://html.spec.whatwg.org#fire-navigate-prr-navigationtype)* set to *historyHandling*, *[isSameDocument](https://html.spec.whatwg.org#fire-navigate-prr-issamedocument)* set to false, *[userInvolvement](https://html.spec.whatwg.org#fire-navigate-prr-userinvolvement)* set to *userInvolvement*, *[sourceElement](https://html.spec.whatwg.org#fire-navigate-prr-sourceelement)* set to *sourceElement*, *[formDataEntryList](https://html.spec.whatwg.org#fire-navigate-prr-formdataentrylist)* set to *entryListForFiring*, *[destinationURL](https://html.spec.whatwg.org#fire-navigate-prr-destinationurl)* set to *url*, *[navigationAPIState](https://html.spec.whatwg.org#fire-navigate-prr-navigationapistate)* set to *navigationAPIStateForFiring*, and *[apiMethodTracker](https://html.spec.whatwg.org#fire-navigate-prr-api-method-tracker)* set to *apiMethodTracker*.
    5. If *continue* is false, then return.


    > **Note:** It is possible for navigations with *userInvolvement* of "[`browser UI`](https://html.spec.whatwg.org#uni-browser-ui)" or initiated by a [cross origin-domain](https://html.spec.whatwg.org#same-origin-domain) *sourceDocument* to fire [`navigate`](https://html.spec.whatwg.org#event-navigate) events, if they go through the earlier [navigate to a fragment](https://html.spec.whatwg.org#navigate-fragid) path.
23. If *sourceDocument* is *navigable*'s [container document](https://html.spec.whatwg.org#nav-container-document), then [reserve deferred fetch quota](https://fetch.spec.whatwg.org/#reserve-deferred-fetch-quota) for *navigable*'s [container](https://html.spec.whatwg.org#nav-container) given *url*'s [origin](https://url.spec.whatwg.org/#concept-url-origin).
24. [In parallel](https://html.spec.whatwg.org#in-parallel), run these steps:

    1. Let *unloadPromptCanceled* be the result of [checking if unloading is canceled](https://html.spec.whatwg.org#checking-if-unloading-is-canceled) for *navigable*'s [active document](https://html.spec.whatwg.org#nav-document)'s [inclusive descendant navigables](https://html.spec.whatwg.org#inclusive-descendant-navigables).
    2. If *unloadPromptCanceled* is not "`continue`", or *navigable*'s [ongoing navigation](https://html.spec.whatwg.org#ongoing-navigation) is no longer *navigationId*:

        1. Invoke [WebDriver BiDi navigation failed](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-failed) with *navigable* and a new [WebDriver BiDi navigation status](https://w3c.github.io/webdriver-bidi/#webdriver-bidi-navigation-status) whose [id](https://w3c.github.io/webdriver-bidi/#navigation-status-id) is *navigationId*, [status](https://w3c.github.io/webdriver-bidi/#navigation-status-status) is "[`canceled`](https://w3c.github.io/webdriver-bidi/#navigation-status-canceled)", and [url](https://w3c.github.io/webdriver-bidi/#navigation-status-url) is *url*.
        2. Abort these steps.
    3. [Queue a global task](https://html.spec.whatwg.org#queue-a-global-task) on the [navigation and traversal task source](https://html.spec.whatwg.org#navigation-and-traversal-task-source) given *navigable*'s [active window](https://html.spec.whatwg.org#nav-window) to [abort a document and its descendants](https://html.spec.whatwg.org#abort-a-document-and-its-descendants) given *navigable*'s [active document](https://html.spec.whatwg.org#nav-document).
    4. Let *documentState* be a new [document state](https://html.spec.whatwg.org#document-state-2) with

        | Field | Value |
        |-------|-------|
        | request referrer policy | referrerPolicy |
        | initiator origin | initiatorOriginSnapshot |
        | resource | documentResource |
        | navigable target name | navigable's target name |

        > **Note:** The [navigable target name](https://html.spec.whatwg.org#document-state-nav-target-name) can get cleared under various conditions later in the navigation process, before the document state is finalized.
    5. If *url* [matches `about:blank`](https://html.spec.whatwg.org#matches-about:blank) or is [`about:srcdoc`](https://html.spec.whatwg.org#about:srcdoc):

        1. Set *documentState*'s [origin](https://html.spec.whatwg.org#document-state-origin) to *initiatorOriginSnapshot*.
        2. Set *documentState*'s [about base URL](https://html.spec.whatwg.org#document-state-about-base-url) to *initiatorBaseURLSnapshot*.
    6. Let *historyEntry* be a new [session history entry](https://html.spec.whatwg.org#session-history-entry), with its [URL](https://html.spec.whatwg.org#she-url) set to *url* and its [document state](https://html.spec.whatwg.org#she-document-state) set to *documentState*.
    7. Let *navigationParams* be null.
    8. If *response* is non-null:

        > **Note:** The [navigate](https://html.spec.whatwg.org#navigate) algorithm is only supplied with a [response](https://fetch.spec.whatwg.org/#concept-response) as part of the [`object`](https://html.spec.whatwg.org#the-object-element) and [`embed`](https://html.spec.whatwg.org#the-embed-element) processing models, or for processing parts of [`multipart/x-mixed-replace` responses](https://html.spec.whatwg.org#navigate-multipart-x-mixed-replace) after the initial response.

        1. Let *sourcePolicyContainer* be a [clone](https://html.spec.whatwg.org#clone-a-policy-container) of the *sourceDocument*'s [policy container](https://html.spec.whatwg.org#concept-document-policy-container), if *sourceDocument* is not null; otherwise null.
        2. Let *policyContainer* be the result of [determining navigation params policy container](https://html.spec.whatwg.org#determining-navigation-params-policy-container) given *response*'s [URL](https://fetch.spec.whatwg.org/#concept-response-url), null, *sourcePolicyContainer*, *navigable*'s [container document](https://html.spec.whatwg.org#nav-container-document)'s [policy container](https://html.spec.whatwg.org#concept-document-policy-container), and null.
        3. Let *finalSandboxFlags* be the [union](https://infra.spec.whatwg.org/#set-union) of *targetSnapshotParams*'s [sandboxing flags](https://html.spec.whatwg.org#target-snapshot-params-sandbox) and *policyContainer*'s [CSP list](https://html.spec.whatwg.org#policy-container-csp-list)'s [CSP-derived sandboxing flags](https://html.spec.whatwg.org#csp-derived-sandboxing-flags).
        4. Let *responseOrigin* be the result of [determining the origin](https://html.spec.whatwg.org#determining-the-origin) given *response*'s [URL](https://fetch.spec.whatwg.org/#concept-response-url), *finalSandboxFlags*, and *documentState*'s [initiator origin](https://html.spec.whatwg.org#document-state-initiator-origin).
        5. Let *coop* be a new [opener policy](https://html.spec.whatwg.org#cross-origin-opener-policy).
        6. Let *coopEnforcementResult* be a new [opener policy enforcement result](https://html.spec.whatwg.org#coop-enforcement-result) with

            | Field | Value |
            |-------|-------|
            | url | response's URL |
            | origin | responseOrigin |
            | opener policy | coop |
        7. Set *navigationParams* to a new [navigation params](https://html.spec.whatwg.org#navigation-params), with

            | Field | Value |
            |-------|-------|
            | id | navigationId |
            | navigable | navigable |
            | request | null |
            | response | response |
            | fetch controller | null |
            | commit early hints | null |
            | COOP enforcement result | coopEnforcementResult |
            | reserved environment | null |
            | origin | responseOrigin |
            | policy container | policyContainer |
            | final sandboxing flag set | finalSandboxFlags |
            | iframe element referrer policy | targetSnapshotParams's iframe element referrer          policy |
            | opener policy | coop |
            | navigation timing type | "navigate" |
            | about base URL | documentState's about base          URL |
            | user involvement | userInvolvement |
    9. [Attempt to populate the history entry's document](https://html.spec.whatwg.org#attempt-to-populate-the-history-entry's-document) for *historyEntry*, given *navigable*, "[`navigate`](https://w3c.github.io/navigation-timing/#dom-navigationtimingtype-navigate)", *sourceSnapshotParams*, *targetSnapshotParams*, *userInvolvement*, *navigationId*, *navigationParams*, *cspNavigationType*, with *[allowPOST](https://html.spec.whatwg.org#attempt-to-populate-allow-post)* set to true and *[completionSteps](https://html.spec.whatwg.org#attempt-to-populate-completion-steps)* set to the following step:

        1. [Append session history traversal steps](https://html.spec.whatwg.org#tn-append-session-history-traversal-steps) to *navigable*'s [traversable navigable](https://html.spec.whatwg.org#nav-traversable) to [finalize a cross-document navigation](https://html.spec.whatwg.org#finalize-a-cross-document-navigation) given *navigable*, *historyHandling*, *userInvolvement*, and *historyEntry*.
