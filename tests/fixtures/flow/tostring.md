The abstract operation ToString takes argument *arg* (an [ECMAScript language value](https://tc39.es/ecma262#sec-ecmascript-language-types)) and returns either a [normal completion containing](https://tc39.es/ecma262#sec-completion-record-specification-type) a String or a [throw completion](https://tc39.es/ecma262#sec-completion-record-specification-type). It converts *arg* to a value of type String. It performs the following steps when called:

1. If *arg* [is a String](https://tc39.es/ecma262#sec-ecmascript-language-types-string-type), return *arg*.
2. If *arg* [is a Symbol](https://tc39.es/ecma262#sec-ecmascript-language-types-symbol-type), throw a TypeError exception.
3. If *arg* is undefined, return "undefined".
4. If *arg* is null, return "null".
5. If *arg* is true, return "true".
6. If *arg* is false, return "false".
7. If *arg* [is a Number](https://tc39.es/ecma262#sec-ecmascript-language-types-number-type), return [Number::toString](https://tc39.es/ecma262#sec-numeric-types-number-tostring)(*arg*, 10).
8. If *arg* [is a BigInt](https://tc39.es/ecma262#sec-ecmascript-language-types-bigint-type), return [BigInt::toString](https://tc39.es/ecma262#sec-numeric-types-bigint-tostring)(*arg*, 10).
9. [Assert](https://tc39.es/ecma262#assert): *arg* [is an Object](https://tc39.es/ecma262#sec-object-type).
10. Let *primitiveValue* be ? [ToPrimitive](https://tc39.es/ecma262#sec-toprimitive)(*arg*, string).
11. [Assert](https://tc39.es/ecma262#assert): *primitiveValue* [is not an Object](https://tc39.es/ecma262#sec-object-type).
12. Return ? [ToString](https://tc39.es/ecma262#sec-tostring)(*primitiveValue*).
