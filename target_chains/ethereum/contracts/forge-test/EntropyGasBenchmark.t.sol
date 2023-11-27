// SPDX-License-Identifier: Apache 2

pragma solidity ^0.8.0;

import "forge-std/Test.sol";
import "@pythnetwork/entropy-sdk-solidity/EntropyStructs.sol";
import "../contracts/entropy/Entropy.sol";
import "./utils/EntropyTestUtils.t.sol";

// TODO
// - what's the impact of # of in-flight requests on gas usage? More requests => more hashes to
//   verify the provider's value.
// - fuzz test?
contract EntropyGasBenchmark is Test, EntropyTestUtils {
    Entropy public random;

    uint pythFeeInWei = 7;

    address public provider1 = address(1);
    bytes32[] provider1Proofs;
    uint provider1FeeInWei = 8;
    uint64 provider1ChainLength = 100;

    address public user1 = address(3);

    function setUp() public {
        random = new Entropy(pythFeeInWei);

        bytes32[] memory hashChain1 = generateHashChain(
            provider1,
            0,
            provider1ChainLength
        );
        provider1Proofs = hashChain1;
        vm.prank(provider1);
        random.register(
            provider1FeeInWei,
            provider1Proofs[0],
            hex"0100",
            provider1ChainLength
        );

        // Register twice so the commitment sequence number is nonzero. Zero values can be misleading
        // when gas benchmarking.
        vm.prank(provider1);
        random.register(
            provider1FeeInWei,
            provider1Proofs[0],
            hex"0100",
            provider1ChainLength
        );

        assert(
            random.getProviderInfo(provider1).currentCommitmentSequenceNumber !=
                0
        );
    }

    // Test helper method for requesting a random value as user from provider.
    function requestHelper(
        address user,
        address provider,
        uint randomNumber,
        bool useBlockhash
    ) public returns (uint64 sequenceNumber) {
        uint fee = random.getFee(provider);
        vm.deal(user, fee);
        vm.prank(user);
        sequenceNumber = random.request{value: fee}(
            provider,
            random.constructUserCommitment(bytes32(randomNumber)),
            useBlockhash
        );
    }

    function testBenchmarkBasicFlow() public {
        uint userRandom = 42;
        uint64 sequenceNumber = requestHelper(
            user1,
            provider1,
            userRandom,
            true
        );

        random.reveal(
            provider1,
            sequenceNumber,
            bytes32(userRandom),
            provider1Proofs[sequenceNumber]
        );
    }
}
