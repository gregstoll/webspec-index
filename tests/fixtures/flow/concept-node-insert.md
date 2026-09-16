To **insert** a [node](https://dom.spec.whatwg.org#concept-node) *node* into a [node](https://dom.spec.whatwg.org#concept-node) *parent* before null or a [node](https://dom.spec.whatwg.org#concept-node) *child*, with an optional boolean ***suppressObservers*** (default false):

1. Let *nodes* be *node*’s [children](https://dom.spec.whatwg.org#concept-tree-child), if *node* is a [`DocumentFragment`](https://dom.spec.whatwg.org#documentfragment) [node](https://dom.spec.whatwg.org#concept-node); otherwise « *node* ».
2. Let *count* be *nodes*’s [size](https://infra.spec.whatwg.org/#list-size).
3. If *count* is 0, then return.
4. If *node* is a [`DocumentFragment`](https://dom.spec.whatwg.org#documentfragment) [node](https://dom.spec.whatwg.org#concept-node):

    1. [Remove](https://dom.spec.whatwg.org#concept-node-remove) its [children](https://dom.spec.whatwg.org#concept-tree-child) with [*suppressObservers*](https://dom.spec.whatwg.org#remove-suppressobservers) set to true.
    2. [Queue a tree mutation record](https://dom.spec.whatwg.org#queue-a-tree-mutation-record) for *node* with « », *nodes*, null, and null.

        > **Note:** This step intentionally does not pay attention to *suppressObservers*.
5. If *child* is non-null:

    1. For each [live range](https://dom.spec.whatwg.org#concept-live-range) whose [start node](https://dom.spec.whatwg.org#concept-range-start-node) is *parent* and [start offset](https://dom.spec.whatwg.org#concept-range-start-offset) is greater than *child*’s [index](https://dom.spec.whatwg.org#concept-tree-index): increase its [start offset](https://dom.spec.whatwg.org#concept-range-start-offset) by *count*.
    2. For each [live range](https://dom.spec.whatwg.org#concept-live-range) whose [end node](https://dom.spec.whatwg.org#concept-range-end-node) is *parent* and [end offset](https://dom.spec.whatwg.org#concept-range-end-offset) is greater than *child*’s [index](https://dom.spec.whatwg.org#concept-tree-index): increase its [end offset](https://dom.spec.whatwg.org#concept-range-end-offset) by *count*.
6. Let *previousSibling* be *child*’s [previous sibling](https://dom.spec.whatwg.org#concept-tree-previous-sibling) or *parent*’s [last child](https://dom.spec.whatwg.org#concept-tree-last-child) if *child* is null.
7. For each *node* in *nodes*, in [tree order](https://dom.spec.whatwg.org#concept-tree-order):

    1. [Adopt](https://dom.spec.whatwg.org#concept-node-adopt) *node* into *parent*’s [node document](https://dom.spec.whatwg.org#concept-node-document).
    2. If *child* is null, then [append](https://infra.spec.whatwg.org/#set-append) *node* to *parent*’s [children](https://dom.spec.whatwg.org#concept-tree-child).
    3. Otherwise, [insert](https://infra.spec.whatwg.org/#list-insert) *node* into *parent*’s [children](https://dom.spec.whatwg.org#concept-tree-child) before *child*’s [index](https://dom.spec.whatwg.org#concept-tree-index).
    4. If *parent* is a [shadow host](https://dom.spec.whatwg.org#element-shadow-host) whose [shadow root](https://dom.spec.whatwg.org#concept-shadow-root)’s [slot assignment](https://dom.spec.whatwg.org#shadowroot-slot-assignment) is "`named`" and *node* is a [slottable](https://dom.spec.whatwg.org#concept-slotable), then [assign a slot](https://dom.spec.whatwg.org#assign-a-slot) for *node*.
    5. If *parent*’s [root](https://dom.spec.whatwg.org#concept-tree-root) is a [shadow root](https://dom.spec.whatwg.org#concept-shadow-root), and *parent* is a [slot](https://dom.spec.whatwg.org#concept-slot) whose [assigned nodes](https://dom.spec.whatwg.org#slot-assigned-nodes) is the empty list, then run [signal a slot change](https://dom.spec.whatwg.org#signal-a-slot-change) for *parent*.
    6. Run [assign slottables for a tree](https://dom.spec.whatwg.org#assign-slotables-for-a-tree) with *node*’s [root](https://dom.spec.whatwg.org#concept-tree-root).
    7. For each [shadow-including inclusive descendant](https://dom.spec.whatwg.org#concept-shadow-including-inclusive-descendant) *inclusiveDescendant* of *node*, in [shadow-including tree order](https://dom.spec.whatwg.org#concept-shadow-including-tree-order):

        1. Run the [insertion steps](https://dom.spec.whatwg.org#concept-node-insert-ext) with *inclusiveDescendant*.
        2. If *inclusiveDescendant* is not [connected](https://dom.spec.whatwg.org#connected), then [continue](https://infra.spec.whatwg.org/#iteration-continue).
        3. If *inclusiveDescendant* is an [element](https://dom.spec.whatwg.org#concept-element) and *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#element-custom-element-registry) is non-null:

            1. If *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#element-custom-element-registry)’s [is scoped](https://html.spec.whatwg.org/multipage/custom-elements.html#is-scoped) is true, then [append](https://infra.spec.whatwg.org/#set-append) *inclusiveDescendant*’s [node document](https://dom.spec.whatwg.org#concept-node-document) to *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#element-custom-element-registry)’s [scoped document set](https://html.spec.whatwg.org/multipage/custom-elements.html#scoped-document-set).
            2. If *inclusiveDescendant* is [custom](https://dom.spec.whatwg.org#concept-element-custom), then [enqueue a custom element callback reaction](https://html.spec.whatwg.org/multipage/custom-elements.html#enqueue-a-custom-element-callback-reaction) with *inclusiveDescendant*, callback name "`connectedCallback`", and « ».
            3. Otherwise, [try to upgrade](https://html.spec.whatwg.org/multipage/custom-elements.html#concept-try-upgrade) *inclusiveDescendant*.

                > **Note:** If this successfully upgrades *inclusiveDescendant*, its `connectedCallback` will be enqueued automatically during the [upgrade an element](https://html.spec.whatwg.org/multipage/custom-elements.html#concept-upgrade-an-element) algorithm.
        4. Otherwise, if *inclusiveDescendant* is a [shadow root](https://dom.spec.whatwg.org#concept-shadow-root), *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#shadowroot-custom-element-registry) is non-null, and *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#shadowroot-custom-element-registry)’s [is scoped](https://html.spec.whatwg.org/multipage/custom-elements.html#is-scoped) is true, then [append](https://infra.spec.whatwg.org/#set-append) *inclusiveDescendant*’s [node document](https://dom.spec.whatwg.org#concept-node-document) to *inclusiveDescendant*’s [custom element registry](https://dom.spec.whatwg.org#shadowroot-custom-element-registry)’s [scoped document set](https://html.spec.whatwg.org/multipage/custom-elements.html#scoped-document-set).
8. If *suppressObservers* is false, then [queue a tree mutation record](https://dom.spec.whatwg.org#queue-a-tree-mutation-record) for *parent* with *nodes*, « », *previousSibling*, and *child*.
9. Run the [children changed steps](https://dom.spec.whatwg.org#concept-node-children-changed-ext) for *parent*.
10. Let *staticNodeList* be a [list](https://infra.spec.whatwg.org/#list) of [nodes](https://dom.spec.whatwg.org#concept-node), initially « ».

    > **Note:** We collect all [nodes](https://dom.spec.whatwg.org#concept-node) *before* calling the [post-connection steps](https://dom.spec.whatwg.org#concept-node-post-connection-ext) on any one of them, instead of calling the [post-connection steps](https://dom.spec.whatwg.org#concept-node-post-connection-ext) *while* we’re traversing the [node tree](https://dom.spec.whatwg.org#concept-node-tree). This is because the [post-connection steps](https://dom.spec.whatwg.org#concept-node-post-connection-ext) can modify the tree’s structure, making live traversal unsafe, possibly leading to the [post-connection steps](https://dom.spec.whatwg.org#concept-node-post-connection-ext) being called multiple times on the same [node](https://dom.spec.whatwg.org#concept-node).
11. For each *node* of *nodes*, in [tree order](https://dom.spec.whatwg.org#concept-tree-order):

    1. For each [shadow-including inclusive descendant](https://dom.spec.whatwg.org#concept-shadow-including-inclusive-descendant) *inclusiveDescendant* of *node*, in [shadow-including tree order](https://dom.spec.whatwg.org#concept-shadow-including-tree-order): [append](https://infra.spec.whatwg.org/#list-append) *inclusiveDescendant* to *staticNodeList*.
12. [For each](https://infra.spec.whatwg.org/#list-iterate) *node* of *staticNodeList*: if *node* is [connected](https://dom.spec.whatwg.org#connected), then run the [post-connection steps](https://dom.spec.whatwg.org#concept-node-post-connection-ext) with *node*.
